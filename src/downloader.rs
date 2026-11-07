//! Hàng đợi tải video, dùng yt-dlp làm engine.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use eframe::egui;
use serde_json::Value;

/// Số video tải song song.
const MAX_PARALLEL: usize = 3;

/// <thư mục videos>/<tên kênh>/<tiêu đề> [<id>].<ext>
const OUTPUT_TEMPLATE: &str = "%(channel,uploader,uploader_id,creator|unknown)s/%(title).150B [%(id)s].%(ext)s";
/// Ưu tiên mp4 + m4a để mở được bằng mọi trình phát.
const FORMAT: &str = "bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/bv*+ba/b";
/// Thông tin video, in ra 1 dòng JSON trước khi tải.
const INFO_TEMPLATE: &str = "before_dl:[I]%(.{channel,uploader,description,tags,duration,view_count,like_count,comment_count,upload_date})j";
const PROGRESS_TEMPLATE: &str = "download:[P]%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed,
}

pub struct Job {
    pub id: usize,
    pub url: String,
    pub title: String,
    pub status: Status,
    pub progress: f32,
    pub speed: String,
    pub eta: String,
    pub file: Option<String>,
    pub error: Option<String>,
    pub attempts: u32,
    pub info: Option<VideoInfo>,
    kind: Kind,
    cookies: Option<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct VideoInfo {
    pub channel: String,
    pub description: String,
    pub hashtags: Vec<String>,
    /// Giây.
    pub duration: Option<f64>,
    pub views: Option<u64>,
    pub likes: Option<u64>,
    pub comments: Option<u64>,
    /// YYYYMMDD.
    pub upload_date: Option<String>,
}

impl VideoInfo {
    fn parse(json: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(json).ok()?;
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_owned();
        let channel = Some(text("channel")).filter(|c| !c.is_empty()).unwrap_or_else(|| text("uploader"));
        let description = text("description");
        let tags = v.get("tags").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str);
        Some(Self {
            channel,
            hashtags: hashtags(&description, tags),
            description,
            duration: v.get("duration").and_then(Value::as_f64),
            views: v.get("view_count").and_then(Value::as_u64),
            likes: v.get("like_count").and_then(Value::as_u64),
            comments: v.get("comment_count").and_then(Value::as_u64),
            upload_date: Some(text("upload_date")).filter(|d| d.len() == 8),
        })
    }
}

/// Hashtag trong mô tả (kể cả dính liền "#a#b"), sau đó tới tags của video. Tối đa 15.
fn hashtags<'a>(description: &'a str, tags: impl Iterator<Item = &'a str>) -> Vec<String> {
    let from_desc = description
        .split_whitespace()
        .filter(|w| w.starts_with('#'))
        .flat_map(|w| w.split('#'))
        .map(|t| t.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_'));
    let mut out: Vec<String> = Vec::new();
    for tag in from_desc.chain(tags) {
        let tag = tag.trim().replace(' ', "");
        if !tag.is_empty() && !out.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
            out.push(tag);
        }
    }
    out.truncate(15);
    out
}

#[derive(Clone, Copy)]
enum Kind {
    Video,
    /// Bước lấy danh sách bị lỗi; retry sẽ liệt kê lại với giới hạn này.
    Listing(Option<usize>),
}

#[derive(Default)]
pub struct State {
    pub jobs: Vec<Job>,
    /// Số link đang được liệt kê (kênh có thể mất vài giây).
    pub listing: usize,
    pub message: String,
    next_id: usize,
}

impl State {
    fn push(&mut self, url: String, title: String, kind: Kind, cookies: Option<String>, error: Option<String>) {
        let id = self.next_id;
        self.next_id += 1;
        let failed = error.is_some();
        self.jobs.push(Job {
            id,
            url,
            title,
            status: if failed { Status::Failed } else { Status::Queued },
            progress: 0.0,
            speed: String::new(),
            eta: String::new(),
            file: None,
            error,
            attempts: u32::from(failed),
            info: None,
            kind,
            cookies,
        });
    }
}

pub struct Manager {
    pub state: Mutex<State>,
    pub out_dir: PathBuf,
    wake: Condvar,
    ctx: egui::Context,
}

impl Manager {
    pub fn new(ctx: egui::Context, out_dir: PathBuf) -> Arc<Self> {
        let m = Arc::new(Self { state: Mutex::default(), out_dir, wake: Condvar::new(), ctx });
        for _ in 0..MAX_PARALLEL {
            let m = Arc::clone(&m);
            thread::spawn(move || m.worker());
        }
        m
    }

    /// Liệt kê video từ `url` (video, kênh, tab, playlist) rồi đưa vào hàng đợi.
    /// `limit = None` nghĩa là tất cả.
    pub fn add_url(self: &Arc<Self>, url: String, limit: Option<usize>, cookies: Option<String>) {
        self.state.lock().unwrap().listing += 1;
        let m = Arc::clone(self);
        thread::spawn(move || {
            let result = list_videos(&url, limit, cookies.as_deref());
            let mut st = m.state.lock().unwrap();
            st.listing -= 1;
            match result {
                Ok(videos) if !videos.is_empty() => {
                    st.message = format!("Đã thêm {} video vào hàng đợi", videos.len());
                    for (url, title) in videos {
                        st.push(url, title, Kind::Video, cookies.clone(), None);
                    }
                    m.wake.notify_all();
                }
                result => {
                    let err = result.err().unwrap_or_else(|| "Không tìm thấy video nào".into());
                    st.message = format!("Lỗi khi lấy danh sách: {err}");
                    let title = format!("Lấy danh sách: {url}");
                    st.push(url, title, Kind::Listing(limit), cookies, Some(err));
                }
            }
            drop(st);
            m.ctx.request_repaint();
        });
    }

    pub fn retry(self: &Arc<Self>, ids: &[usize]) {
        let mut relist = Vec::new();
        {
            let mut st = self.state.lock().unwrap();
            st.jobs.retain_mut(|j| {
                if !ids.contains(&j.id) || j.status != Status::Failed {
                    return true;
                }
                match j.kind {
                    Kind::Video => {
                        j.status = Status::Queued;
                        j.error = None;
                        true
                    }
                    Kind::Listing(limit) => {
                        relist.push((j.url.clone(), limit, j.cookies.clone()));
                        false
                    }
                }
            });
            self.wake.notify_all();
        }
        for (url, limit, cookies) in relist {
            self.add_url(url, limit, cookies);
        }
    }

    fn with_job(&self, id: usize, f: impl FnOnce(&mut Job)) {
        if let Some(j) = self.state.lock().unwrap().jobs.iter_mut().find(|j| j.id == id) {
            f(j);
        }
        self.ctx.request_repaint();
    }

    fn worker(&self) {
        loop {
            let (id, url, cookies) = {
                let mut st = self.state.lock().unwrap();
                loop {
                    if let Some(j) = st.jobs.iter_mut().find(|j| j.status == Status::Queued) {
                        j.status = Status::Running;
                        j.progress = 0.0;
                        j.attempts += 1;
                        break (j.id, j.url.clone(), j.cookies.clone());
                    }
                    st = self.wake.wait(st).unwrap();
                }
            };
            self.ctx.request_repaint();
            let result = self.download(id, &url, cookies.as_deref());
            self.with_job(id, |j| match result {
                Ok(file) => {
                    j.status = Status::Done;
                    j.progress = 1.0;
                    j.file = file;
                }
                Err(e) => {
                    j.status = Status::Failed;
                    j.error = Some(e);
                }
            });
        }
    }

    /// Tải 1 video, cập nhật tiến trình theo từng dòng output của yt-dlp.
    fn download(&self, id: usize, url: &str, cookies: Option<&str>) -> Result<Option<String>, String> {
        let mut cmd = Command::new("yt-dlp");
        cmd.args(["--no-playlist", "--newline", "--no-colors", "--progress", "--no-simulate"])
            .args(["--progress-template", PROGRESS_TEMPLATE])
            .args(["--print", "before_dl:[T]%(title)s", "--print", "after_move:[F]%(filepath)s"])
            .args(["--print", INFO_TEMPLATE])
            .args(["-f", FORMAT, "-o", OUTPUT_TEMPLATE])
            .arg("-P")
            .arg(&self.out_dir);
        if let Some(browser) = cookies {
            cmd.args(["--cookies-from-browser", browser]);
        }
        // "--" để URL không bao giờ bị hiểu thành option.
        cmd.arg("--").arg(url).stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| format!("Không chạy được yt-dlp: {e}"))?;
        let stderr = child.stderr.take().expect("stderr piped");
        let stderr_reader = thread::spawn(move || BufReader::new(stderr).lines().map_while(Result::ok).collect::<Vec<_>>());

        let mut file = None;
        let stdout = child.stdout.take().expect("stdout piped");
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(p) = line.strip_prefix("[P]") {
                let (progress, speed, eta) = parse_progress(p);
                self.with_job(id, |j| {
                    if let Some(p) = progress {
                        j.progress = p;
                    }
                    j.speed = speed;
                    j.eta = eta;
                });
            } else if let Some(t) = line.strip_prefix("[T]") {
                let t = t.to_owned();
                self.with_job(id, |j| j.title = t);
            } else if let Some(info) = line.strip_prefix("[I]").and_then(VideoInfo::parse) {
                self.with_job(id, |j| j.info = Some(info));
            } else if let Some(f) = line.strip_prefix("[F]") {
                file = Some(f.to_owned());
            }
        }

        let status = child.wait().map_err(|e| e.to_string())?;
        let errors = stderr_reader.join().unwrap_or_default();
        if status.success() {
            Ok(file)
        } else {
            Err(last_error(&errors).unwrap_or_else(|| format!("yt-dlp thoát với {status}")))
        }
    }
}

/// Gọi `yt-dlp --flat-playlist -J` để lấy danh sách (url, tiêu đề) mà không tải.
fn list_videos(url: &str, limit: Option<usize>, cookies: Option<&str>) -> Result<Vec<(String, String)>, String> {
    let mut cmd = Command::new("yt-dlp");
    cmd.args(["--flat-playlist", "-J", "--no-colors"]);
    if let Some(n) = limit {
        cmd.arg("-I").arg(format!("1:{n}"));
    }
    if limit == Some(1) {
        // Link dạng watch?v=..&list=.. thì chỉ lấy đúng video đó.
        cmd.arg("--no-playlist");
    }
    if let Some(browser) = cookies {
        cmd.args(["--cookies-from-browser", browser]);
    }
    let out = cmd.arg("--").arg(url).output().map_err(|e| format!("Không chạy được yt-dlp: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let lines: Vec<String> = stderr.lines().map(str::to_owned).collect();
        return Err(last_error(&lines).unwrap_or_else(|| format!("yt-dlp thoát với {}", out.status)));
    }
    let json: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("JSON không hợp lệ: {e}"))?;

    let mut videos = Vec::new();
    collect_entries(&json, &mut videos);
    let mut seen = std::collections::HashSet::new();
    videos.retain(|(u, _)| seen.insert(u.clone()));
    if let Some(n) = limit {
        // Kênh YouTube gốc trả về nhiều tab (Videos, Live, Shorts), mỗi tab tối đa n.
        videos.truncate(n);
    }
    Ok(videos)
}

/// Duyệt đệ quy: playlist/kênh có `entries` (có thể lồng nhau), video thì có url.
fn collect_entries(v: &Value, out: &mut Vec<(String, String)>) {
    if let Some(entries) = v.get("entries").and_then(Value::as_array) {
        for e in entries {
            collect_entries(e, out);
        }
        return;
    }
    let url = v.get("webpage_url").or_else(|| v.get("url")).and_then(Value::as_str);
    if let Some(url) = url {
        let title = v.get("title").and_then(Value::as_str).unwrap_or(url);
        out.push((url.to_owned(), title.to_owned()));
    }
}

fn last_error(lines: &[String]) -> Option<String> {
    lines
        .iter()
        .rev()
        .find(|l| l.starts_with("ERROR:"))
        .or_else(|| lines.iter().rev().find(|l| !l.trim().is_empty()))
        .cloned()
}

/// "downloaded|total|estimate|speed|eta" ("NA" nếu không có) -> (tỉ lệ, tốc độ, ETA).
fn parse_progress(s: &str) -> (Option<f32>, String, String) {
    let f: Vec<Option<f64>> = s.split('|').map(|x| x.parse().ok()).collect();
    let get = |i: usize| f.get(i).copied().flatten();
    let progress = match (get(0), get(1).or(get(2))) {
        (Some(done), Some(total)) if total > 0.0 => Some((done / total).clamp(0.0, 1.0) as f32),
        _ => None,
    };
    let speed = get(3).map(|b| format!("{}/s", human_bytes(b))).unwrap_or_default();
    let eta = get(4).map(|s| format!("{:02}:{:02}", s as u64 / 60, s as u64 % 60)).unwrap_or_default();
    (progress, speed, eta)
}

fn human_bytes(mut v: f64) -> String {
    let units = ["B", "KiB", "MiB", "GiB"];
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", units[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_parsing() {
        let (p, speed, eta) = parse_progress("1024|2048|NA|1048576|75");
        assert_eq!(p, Some(0.5));
        assert_eq!(speed, "1.0 MiB/s");
        assert_eq!(eta, "01:15");
        // Không có total -> dùng estimate; thiếu hết -> None.
        assert_eq!(parse_progress("50|NA|100|NA|NA").0, Some(0.5));
        assert_eq!(parse_progress("NA|NA|NA|NA|NA"), (None, String::new(), String::new()));
    }

    #[test]
    fn nested_channel_tabs_are_flattened() {
        let json: Value = serde_json::json!({
            "_type": "playlist",
            "entries": [
                {"_type": "playlist", "title": "Videos", "entries": [
                    {"_type": "url", "url": "https://y/v1", "title": "a"},
                    null,
                ]},
                {"_type": "playlist", "title": "Shorts", "entries": [
                    {"_type": "url", "url": "https://y/s1", "title": "b"},
                ]},
            ]
        });
        let mut out = Vec::new();
        collect_entries(&json, &mut out);
        assert_eq!(out, vec![("https://y/v1".into(), "a".into()), ("https://y/s1".into(), "b".into())]);
    }

    /// Cần mạng + yt-dlp: `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn end_to_end_download_and_retry() {
        let dir = std::env::temp_dir().join(format!("vd-e2e-{}", std::process::id()));
        let m = Manager::new(egui::Context::default(), dir.clone());
        m.add_url("https://www.youtube.com/@jawed".into(), Some(1), None);
        m.add_url("https://www.youtube.com/watch?v=xxxxxxxxxxx".into(), Some(1), None);
        m.add_url("not a url".into(), Some(1), None);

        let wait = |pred: &dyn Fn(&State) -> bool| {
            for _ in 0..600 {
                if pred(&m.state.lock().unwrap()) {
                    return;
                }
                thread::sleep(std::time::Duration::from_millis(200));
            }
            panic!("timeout");
        };
        let settled = |s: &State| s.listing == 0 && s.jobs.iter().all(|j| matches!(j.status, Status::Done | Status::Failed));
        wait(&settled);

        let failed_ids = |s: &State| s.jobs.iter().filter(|j| j.status == Status::Failed).map(|j| j.id).collect::<Vec<_>>();
        let (done_id, list_fail) = {
            let st = m.state.lock().unwrap();
            for j in &st.jobs {
                eprintln!("{:?} {} {:?} {:?}", j.status as u8, j.title, j.file, j.error);
            }
            let done = st.jobs.iter().find(|j| j.status == Status::Done).expect("1 video tải xong");
            let file = PathBuf::from(done.file.as_ref().unwrap());
            assert!(file.exists());
            let info = done.info.as_ref().expect("có thông tin video");
            assert_eq!(info.channel, "jawed");
            assert!(info.views.is_some() && info.duration.is_some());
            assert_eq!(file.parent().unwrap().parent().unwrap(), dir, "lưu ở <dir>/<kênh>/");
            (done.id, failed_ids(&st))
        };
        assert_eq!(list_fail.len(), 2, "2 link hỏng phải thành 2 dòng lỗi");

        // Retry lỗi lấy danh sách: liệt kê lại -> vẫn lỗi, nhưng là job mới.
        m.retry(&list_fail);
        wait(&settled);
        let again = failed_ids(&m.state.lock().unwrap());
        assert_eq!(again.len(), 2);
        assert!(again.iter().all(|id| !list_fail.contains(id)));

        // Retry video lỗi: tải lại, lần thử thứ 2.
        m.with_job(done_id, |j| j.status = Status::Failed);
        m.retry(&[done_id]);
        wait(&settled);
        let st = m.state.lock().unwrap();
        let j = st.jobs.iter().find(|j| j.id == done_id).unwrap();
        assert!(j.status == Status::Done && j.attempts == 2);
        drop(st);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn video_info_parsing() {
        let json = r##"{"channel": null, "uploader": "Bếp Nhà", "description": "Phở bò ngon #phở #ẩmthực, #fyp#viral\nxem thêm", "tags": ["nấu ăn", "phở"], "duration": 225, "view_count": 1200, "like_count": null, "upload_date": "20261106"}"##;
        let info = VideoInfo::parse(json).unwrap();
        assert_eq!(info.channel, "Bếp Nhà");
        assert_eq!(info.hashtags, ["phở", "ẩmthực", "fyp", "viral", "nấuăn"]);
        assert_eq!(info.duration, Some(225.0));
        assert_eq!((info.views, info.likes), (Some(1200), None));
        assert_eq!(info.upload_date.as_deref(), Some("20261106"));
        assert!(VideoInfo::parse("not json").is_none());
    }

    #[test]
    fn error_prefers_error_line() {
        let lines = vec!["ERROR: boom".to_string(), "WARNING: later".to_string()];
        assert_eq!(last_error(&lines).as_deref(), Some("ERROR: boom"));
    }
}
