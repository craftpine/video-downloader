mod downloader;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use downloader::{Job, Manager, Status, VideoInfo};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Margin, RichText, Shadow, Stroke, TextFormat, TextStyle,
    Vec2,
    text::{LayoutJob, TextWrapping},
};

/// Trình duyệt để yt-dlp lấy cookie (Douyin, Facebook thường bắt buộc).
const BROWSERS: [&str; 6] = ["chrome", "safari", "firefox", "edge", "brave", "chromium"];
const PLATFORMS: [&str; 5] = ["YouTube", "Shorts", "TikTok", "Douyin", "Facebook Reel"];

/// Font hệ thống có tiếng Việt + CJK, dùng làm fallback cho tiêu đề Douyin/TikTok.
const FALLBACK_FONTS: [&str; 4] = [
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/Library/Fonts/Arial Unicode.ttf",
    "C:\\Windows\\Fonts\\msyh.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
];

// Bảng màu (dark).
const BG: Color32 = Color32::from_rgb(15, 17, 23);
const SIDEBAR: Color32 = Color32::from_rgb(11, 13, 18);
const SURFACE: Color32 = Color32::from_rgb(23, 26, 35);
const SURFACE_2: Color32 = Color32::from_rgb(33, 37, 50);
const HOVER: Color32 = Color32::from_rgb(42, 47, 64);
const BORDER: Color32 = Color32::from_rgb(42, 47, 61);
const TEXT: Color32 = Color32::from_rgb(230, 232, 239);
const MUTED: Color32 = Color32::from_rgb(139, 144, 160);
const ACCENT: Color32 = Color32::from_rgb(99, 102, 241);
const ACCENT_LIGHT: Color32 = Color32::from_rgb(129, 132, 248);
const SUCCESS: Color32 = Color32::from_rgb(34, 197, 94);
const INFO: Color32 = Color32::from_rgb(56, 189, 248);
const DANGER: Color32 = Color32::from_rgb(239, 68, 68);
const IDLE: Color32 = Color32::from_rgb(100, 116, 139);

#[derive(PartialEq)]
enum Tab {
    Download,
    Progress,
}

#[derive(PartialEq)]
enum Mode {
    One,
    Many,
    All,
}

struct App {
    mgr: Arc<Manager>,
    tab: Tab,
    url: String,
    mode: Mode,
    count: usize,
    browser: Option<&'static str>,
    ytdlp_ok: bool,
    /// Các job đang mở rộng phần mô tả.
    expanded: HashSet<usize>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fallback_font(&cc.egui_ctx);
        apply_theme(&cc.egui_ctx);
        let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("videos");
        Self {
            mgr: Manager::new(cc.egui_ctx.clone(), out_dir),
            tab: Tab::Download,
            url: String::new(),
            mode: Mode::One,
            count: 10,
            browser: None,
            ytdlp_ok: Command::new("yt-dlp").arg("--version").output().is_ok_and(|o| o.status.success()),
            expanded: HashSet::new(),
        }
    }

    fn start(&mut self) {
        let url = self.url.trim().to_owned();
        if url.is_empty() {
            return;
        }
        let limit = match self.mode {
            Mode::One => Some(1),
            Mode::Many => Some(self.count),
            Mode::All => None,
        };
        self.mgr.add_url(url, limit, self.browser.map(str::to_owned));
        self.url.clear();
        self.tab = Tab::Progress;
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, running: usize, failed: usize) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(ACCENT)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| ui.label(RichText::new("⬇").size(20.0).color(Color32::WHITE)));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new("Video Downloader").size(17.0).strong().color(TEXT));
                ui.label(RichText::new("yt-dlp · Rust").small().color(MUTED));
            });
        });
        ui.add_space(24.0);

        ui.spacing_mut().item_spacing.y = 6.0;
        if nav_item(ui, self.tab == Tab::Download, "⬇   Tải video", None) {
            self.tab = Tab::Download;
        }
        let counter = match (running, failed) {
            (_, f) if f > 0 => Some((f.to_string(), DANGER)),
            (r, _) if r > 0 => Some((r.to_string(), INFO)),
            _ => None,
        };
        if nav_item(ui, self.tab == Tab::Progress, "☰   Tiến trình", counter) {
            self.tab = Tab::Progress;
        }

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(4.0);
            if self.ytdlp_ok {
                badge(ui, "● yt-dlp sẵn sàng", SUCCESS);
            } else {
                badge(ui, "● Thiếu yt-dlp", DANGER);
            }
            let open = egui::Button::new(RichText::new("🗁   Mở thư mục videos").color(TEXT))
                .fill(SURFACE_2)
                .min_size(Vec2::new(ui.available_width(), 42.0));
            if ui.add(open).clicked() {
                open_path(&self.mgr.out_dir);
            }
        });
    }

    fn download_page(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "Tải video", "Dán link kênh hoặc video, chọn số lượng rồi bắt đầu tải.");
        ui.set_max_width(780.0);

        card().show(ui, |ui| {
            ui.set_width(ui.available_width());

            section_label(ui, "URL kênh hoặc video");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.url)
                    .hint_text("https://www.youtube.com/@kenh   ·   https://vt.tiktok.com/...")
                    .font(FontId::proportional(17.0))
                    .margin(Margin::symmetric(14, 12))
                    .desired_width(f32::INFINITY),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for p in PLATFORMS {
                    badge(ui, p, MUTED);
                }
            });
            ui.add_space(14.0);

            section_label(ui, "Số lượng");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for (mode, text) in [(Mode::One, "1 video"), (Mode::Many, "Nhiều video"), (Mode::All, "Tất cả")] {
                    if toggle(ui, self.mode == mode, text) {
                        self.mode = mode;
                    }
                }
                if self.mode == Mode::Many {
                    ui.add_space(6.0);
                    ui.add_sized([110.0, 42.0], egui::DragValue::new(&mut self.count).range(1..=100_000).suffix(" video"));
                }
            });
            ui.label(
                RichText::new("Link kênh: lấy video mới nhất trước. Link video: luôn tải đúng video đó.")
                    .small()
                    .color(MUTED),
            );
            ui.add_space(14.0);

            section_label(ui, "Cookie từ trình duyệt");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("browser")
                    .width(200.0)
                    .selected_text(self.browser.unwrap_or("Không dùng"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.browser, None, "Không dùng");
                        for b in BROWSERS {
                            ui.selectable_value(&mut self.browser, Some(b), b);
                        }
                    });
                ui.label(RichText::new("Douyin / Facebook thường cần").small().color(MUTED));
            });
            ui.add_space(22.0);

            let can_start = self.ytdlp_ok && !self.url.trim().is_empty();
            let button = egui::Button::new(RichText::new("⬇   Bắt đầu tải").size(17.0).strong().color(Color32::WHITE))
                .fill(ACCENT)
                .corner_radius(CornerRadius::same(12))
                .min_size(Vec2::new(ui.available_width(), 52.0));
            let clicked = ui.add_enabled(can_start, button).clicked();
            if can_start && (clicked || enter) {
                self.start();
            }
        });

        ui.add_space(12.0);
        ui.label(
            RichText::new(format!("Lưu tại  {}/<tên kênh>/", self.mgr.out_dir.display()))
                .small()
                .color(MUTED),
        );
    }

    fn progress_page(&mut self, ui: &mut egui::Ui) {
        let mut retry_ids = Vec::new();
        let mut open: Option<PathBuf> = None;
        let mut toggle_desc = None;
        {
            let st = self.mgr.state.lock().unwrap();
            let count = |s: Status| st.jobs.iter().filter(|j| j.status == s).count();
            let failed = count(Status::Failed);

            ui.horizontal(|ui| {
                page_header(ui, "Tiến trình", "Theo dõi các video đang tải, thử lại nếu lỗi.");
                if failed > 0 {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let b = egui::Button::new(
                            RichText::new(format!("↻   Thử lại tất cả lỗi ({failed})")).strong().color(DANGER),
                        )
                        .fill(DANGER.gamma_multiply(0.15))
                        .stroke(Stroke::new(1.0, DANGER.gamma_multiply(0.5)))
                        .min_size(Vec2::new(0.0, 44.0));
                        if ui.add(b).clicked() {
                            retry_ids.extend(st.jobs.iter().filter(|j| j.status == Status::Failed).map(|j| j.id));
                        }
                    });
                }
            });

            let stats = [
                ("Tổng", st.jobs.len(), TEXT),
                ("Đang tải", count(Status::Running), INFO),
                ("Đang chờ", count(Status::Queued), IDLE),
                ("Hoàn tất", count(Status::Done), SUCCESS),
                ("Lỗi", failed, DANGER),
            ];
            ui.columns(stats.len(), |cols| {
                for (col, (label, n, color)) in cols.iter_mut().zip(stats) {
                    stat_card(col, label, n, color);
                }
            });
            ui.add_space(10.0);

            if st.jobs.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.label(RichText::new("📭").size(48.0));
                    ui.label(RichText::new("Chưa có video nào").size(18.0).strong().color(TEXT));
                    ui.label(RichText::new("Dán link ở trang Tải video để bắt đầu.").color(MUTED));
                    ui.add_space(10.0);
                    let b = egui::Button::new(RichText::new("⬇   Thêm video").strong().color(Color32::WHITE))
                        .fill(ACCENT)
                        .min_size(Vec2::new(180.0, 44.0));
                    if ui.add(b).clicked() {
                        self.tab = Tab::Download;
                    }
                });
            }

            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                for j in &st.jobs {
                    match job_card(ui, j, self.expanded.contains(&j.id)) {
                        Some(CardAction::Retry) => retry_ids.push(j.id),
                        Some(CardAction::ToggleDescription) => toggle_desc = Some(j.id),
                        Some(CardAction::Open) => {
                            open = j.file.as_deref().and_then(|f| Path::new(f).parent()).map(Path::to_path_buf);
                        }
                        None => {}
                    }
                }
            });
        }
        // Gọi sau khi đã nhả lock.
        if let Some(id) = toggle_desc
            && !self.expanded.remove(&id)
        {
            self.expanded.insert(id);
        }
        if !retry_ids.is_empty() {
            self.mgr.retry(&retry_ids);
        }
        if let Some(p) = open {
            open_path(&p);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // winit 0.30 trên macOS mới crash (NSRangeException, Touch Bar) khi dọn cửa sổ lúc đóng.
        // App không có gì cần lưu nên thoát luôn, bỏ qua bước dọn dẹp.
        if ctx.input(|i| i.viewport().close_requested()) {
            std::process::exit(0);
        }
        let (listing, message, running, failed) = {
            let st = self.mgr.state.lock().unwrap();
            let n = |s| st.jobs.iter().filter(|j| j.status == s).count();
            (st.listing, st.message.clone(), n(Status::Running), n(Status::Failed))
        };

        egui::SidePanel::left("nav")
            .exact_width(240.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(SIDEBAR).inner_margin(Margin::same(18)))
            .show(ctx, |ui| self.sidebar(ui, running, failed));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::symmetric(32, 26)))
            .show(ctx, |ui| {
                if !self.ytdlp_ok {
                    banner(ui, DANGER, "Không tìm thấy yt-dlp trong PATH. Cài bằng: brew install yt-dlp", false);
                }
                if listing > 0 {
                    banner(ui, INFO, &format!("Đang lấy danh sách video ({listing})..."), true);
                } else if !message.is_empty() {
                    let color = if message.starts_with("Lỗi") { DANGER } else { SUCCESS };
                    banner(ui, color, &message, false);
                }
                match self.tab {
                    Tab::Download => self.download_page(ui),
                    Tab::Progress => self.progress_page(ui),
                }
            });
    }
}

enum CardAction {
    Retry,
    Open,
    ToggleDescription,
}

fn job_card(ui: &mut egui::Ui, j: &Job, expanded: bool) -> Option<CardAction> {
    let (label, color) = match j.status {
        Status::Queued => ("Đang chờ", IDLE),
        Status::Running => ("Đang tải", INFO),
        Status::Done => ("Hoàn tất", SUCCESS),
        Status::Failed => ("Lỗi", DANGER),
    };
    let mut action = None;
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(18, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                badge(ui, label, color);
                ui.add(egui::Label::new(RichText::new(&j.title).size(15.5).strong().color(TEXT)).truncate())
                    .on_hover_text(&j.url);
            });
            if let Some(info) = &j.info
                && video_info(ui, info, expanded)
            {
                action = Some(CardAction::ToggleDescription);
            }
            if j.info.is_some() {
                ui.add_space(4.0);
            }

            match j.status {
                Status::Queued => {
                    ui.label(RichText::new("Đang chờ đến lượt...").small().color(MUTED));
                }
                Status::Running => {
                    ui.add(
                        egui::ProgressBar::new(j.progress)
                            .desired_height(10.0)
                            .corner_radius(CornerRadius::same(5))
                            .fill(ACCENT),
                    );
                    let mut text = format!("{:.0}%", j.progress * 100.0);
                    for extra in [&j.speed, &j.eta] {
                        if !extra.is_empty() {
                            text.push_str("   ·   ");
                            text.push_str(extra);
                        }
                    }
                    ui.label(RichText::new(text).small().color(MUTED));
                }
                Status::Done => {
                    let file = j.file.as_deref().unwrap_or_default();
                    if action_row(ui, RichText::new(file).small().color(MUTED), file, "🗁   Mở thư mục", SURFACE_2, TEXT) {
                        action = Some(CardAction::Open);
                    }
                }
                Status::Failed => {
                    let err = j.error.as_deref().unwrap_or("Lỗi không rõ");
                    let text = RichText::new(format!("Lần thử {}  ·  {err}", j.attempts)).small().color(DANGER);
                    if action_row(ui, text, err, "↻   Thử lại", ACCENT, Color32::WHITE) {
                        action = Some(CardAction::Retry);
                    }
                }
            }
        });
    action
}

/// Kênh, thời lượng, lượt xem..., hashtag và mô tả (3 dòng, bấm để xem hết).
/// Trả về true khi người dùng bấm "Xem thêm"/"Thu gọn".
fn video_info(ui: &mut egui::Ui, info: &VideoInfo, expanded: bool) -> bool {
    let mut meta = Vec::new();
    if !info.channel.is_empty() {
        meta.push(format!("👤 {}", info.channel));
    }
    if let Some(d) = info.duration {
        meta.push(format!("⏱ {}", format_duration(d)));
    }
    if let Some(n) = info.views {
        meta.push(format!("👁 {} lượt xem", compact(n)));
    }
    if let Some(n) = info.likes {
        meta.push(format!("♥ {}", compact(n)));
    }
    if let Some(n) = info.comments {
        meta.push(format!("💬 {}", compact(n)));
    }
    if let Some(d) = &info.upload_date {
        meta.push(format!("📅 {}/{}/{}", &d[6..8], &d[4..6], &d[..4]));
    }
    if !meta.is_empty() {
        ui.label(RichText::new(meta.join("    ")).size(13.0).color(MUTED));
    }

    if !info.hashtags.is_empty() {
        // Mặc định mỗi dòng cao tối thiểu bằng nút (38px) -> các dòng chip cách nhau quá xa.
        // Phải đặt trước khi tạo horizontal_wrapped vì chiều cao dòng được lấy lúc tạo.
        let original = ui.spacing().interact_size.y;
        ui.spacing_mut().interact_size.y = 0.0;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            for tag in &info.hashtags {
                // Tag quá dài sẽ rộng hơn cả dòng -> cắt bớt.
                let text = match tag.char_indices().nth(28) {
                    Some((i, _)) => format!("#{}…", &tag[..i]),
                    None => format!("#{tag}"),
                };
                pill(ui, &text, ACCENT_LIGHT, ACCENT.gamma_multiply(0.14));
            }
        });
        ui.spacing_mut().interact_size.y = original;
    }

    let mut toggled = false;
    if !info.description.is_empty() {
        let mut job = LayoutJob::single_section(
            info.description.clone(),
            TextFormat { font_id: FontId::proportional(13.5), color: TEXT.gamma_multiply(0.8), ..Default::default() },
        );
        job.wrap = TextWrapping {
            max_rows: if expanded { usize::MAX } else { 3 },
            overflow_character: Some('…'),
            ..TextWrapping::wrap_at_width(ui.available_width())
        };
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let long = galley.elided || expanded;
        ui.label(galley);
        if long && ui.link(if expanded { "Thu gọn" } else { "Xem thêm" }).clicked() {
            toggled = true;
        }
    }
    toggled
}

fn format_duration(secs: f64) -> String {
    let s = secs.round() as u64;
    match (s / 3600, s / 60 % 60, s % 60) {
        (0, m, s) => format!("{m}:{s:02}"),
        (h, m, s) => format!("{h}:{m:02}:{s:02}"),
    }
}

/// 1234 -> 1.2K, 3_400_000 -> 3.4M
fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}K", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}B", n as f64 / 1e9),
    }
}

/// Dòng gồm chữ (cắt bớt nếu dài, rê chuột để xem đủ) + nút bên phải.
fn action_row(ui: &mut egui::Ui, text: RichText, hover: &str, button: &str, fill: Color32, fg: Color32) -> bool {
    let mut clicked = false;
    let size = Vec2::new(ui.available_width(), 38.0);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
        let b = egui::Button::new(RichText::new(button).strong().color(fg))
            .fill(fill)
            .min_size(Vec2::new(140.0, 38.0));
        clicked = ui.add(b).clicked();
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.add(egui::Label::new(text).truncate()).on_hover_text(hover);
        });
    });
    clicked
}

fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(24))
}

fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(RichText::new(title).size(28.0).strong().color(TEXT));
        ui.label(RichText::new(subtitle).color(MUTED));
        ui.add_space(14.0);
    });
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(13.5).strong().color(MUTED));
}

fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    pill(ui, text, color, color.gamma_multiply(0.16));
}

/// Nhãn bo tròn có kích thước cố định theo chữ (không bao giờ xuống dòng bên trong),
/// nên trong `horizontal_wrapped` cả nhãn sẽ được đẩy xuống dòng mới khi hết chỗ.
fn pill(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) {
    let padding = Vec2::new(10.0, 4.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(12.5), fg);
    let (rect, _) = ui.allocate_exact_size(galley.size() + 2.0 * padding, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, CornerRadius::same(255), bg);
        ui.painter().galley(rect.min + padding, galley, fg);
    }
}

fn banner(ui: &mut egui::Ui, color: Color32, text: &str, spinner: bool) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.4)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if spinner {
                    ui.add(egui::Spinner::new().color(color));
                }
                ui.add(egui::Label::new(RichText::new(text).color(color)).truncate());
            });
        });
    ui.add_space(12.0);
}

fn toggle(ui: &mut egui::Ui, selected: bool, text: &str) -> bool {
    let (fill, fg, stroke) = if selected { (ACCENT, Color32::WHITE, ACCENT) } else { (SURFACE_2, TEXT, BORDER) };
    let b = egui::Button::new(RichText::new(text).size(15.0).color(fg))
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(10))
        .min_size(Vec2::new(130.0, 42.0));
    ui.add(b).clicked()
}

fn nav_item(ui: &mut egui::Ui, selected: bool, text: &str, counter: Option<(String, Color32)>) -> bool {
    let (fill, fg) = if selected { (SURFACE_2, TEXT) } else { (Color32::TRANSPARENT, MUTED) };
    let right = match counter {
        Some((n, c)) => RichText::new(n).strong().color(c),
        None => RichText::new(""),
    };
    let b = egui::Button::new(RichText::new(text).size(15.5).color(fg))
        .right_text(right)
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(10))
        .min_size(Vec2::new(ui.available_width(), 46.0));
    ui.add(b).clicked()
}

fn stat_card(ui: &mut egui::Ui, label: &str, n: usize, color: Color32) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(n.to_string()).size(26.0).strong().color(color));
            ui.label(RichText::new(label).small().color(MUTED));
        });
}

fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::proportional(26.0)),
            (TextStyle::Body, FontId::proportional(15.0)),
            (TextStyle::Button, FontId::proportional(15.0)),
            (TextStyle::Small, FontId::proportional(12.5)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        s.spacing.item_spacing = Vec2::new(10.0, 10.0);
        s.spacing.button_padding = Vec2::new(16.0, 9.0);
        s.spacing.interact_size.y = 38.0;

        let v = &mut s.visuals;
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.extreme_bg_color = SURFACE_2;
        v.faint_bg_color = SURFACE;
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.popup_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(140) };

        let w = &mut v.widgets;
        for (state, fill, stroke, fg) in [
            (&mut w.noninteractive, SURFACE, BORDER, TEXT),
            (&mut w.inactive, SURFACE_2, BORDER, TEXT),
            (&mut w.hovered, HOVER, ACCENT_LIGHT, TEXT),
            (&mut w.active, ACCENT, ACCENT, Color32::WHITE),
            (&mut w.open, SURFACE_2, ACCENT, TEXT),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.fg_stroke = Stroke::new(1.5, fg);
            state.corner_radius = CornerRadius::same(10);
            state.expansion = 0.0;
        }
    });
}

fn install_fallback_font(ctx: &egui::Context) {
    let Some(bytes) = FALLBACK_FONTS.iter().find_map(|p| std::fs::read(p).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("fallback".into(), Arc::new(egui::FontData::from_owned(bytes)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("fallback".into());
    }
    ctx.set_fonts(fonts);
}

fn open_path(path: &Path) {
    let _ = std::fs::create_dir_all(path);
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = Command::new(opener).arg(path).spawn();
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 720.0])
            .with_min_inner_size([820.0, 560.0])
            .with_title("Video Downloader"),
        ..Default::default()
    };
    eframe::run_native("Video Downloader", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
