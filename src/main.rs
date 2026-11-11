#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use serde::Deserialize;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

#[cfg(windows)]
use std::collections::HashMap;
#[cfg(windows)]
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
#[cfg(windows)]
use tray_icon::{TrayIcon, TrayIconBuilder};

const REPOSITORY: &str = "Flowseal/zapret-discord-youtube";
const RELEASES_URL: &str =
    "https://api.github.com/repos/Flowseal/zapret-discord-youtube/releases/latest";
const MAX_ARCHIVE_BYTES: u64 = 100 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 300 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

enum BackgroundResult {
    Installed(Result<(String, PathBuf), String>),
    Tested(Result<Option<String>, String>),
}

#[cfg(windows)]
#[derive(Clone)]
enum TrayAction {
    ShowWindow,
    ApplyConfig(String),
    Stop,
    Test,
    Update,
    Service,
    Quit,
}

struct EasyRussiaApp {
    data_dir: PathBuf,
    active_tag: Option<String>,
    best_config: Option<String>,
    available_configs: Vec<String>,
    selected_config: Option<String>,
    active_config: Option<String>,
    startup_enabled: bool,
    is_running: bool,
    last_probe: Instant,
    config_filter: String,
    busy: bool,
    status: String,
    tx: Sender<BackgroundResult>,
    rx: Receiver<BackgroundResult>,
    #[cfg(windows)]
    tray: Option<TrayIcon>,
    #[cfg(windows)]
    tray_initialized: bool,
    #[cfg(windows)]
    tray_actions: HashMap<MenuId, TrayAction>,
    #[cfg(windows)]
    allow_exit: bool,
}

impl EasyRussiaApp {
    fn new() -> Self {
        let data_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("EasyRussia");
        let active_tag = read_active_tag(&data_dir);
        let available_configs = active_tag
            .as_ref()
            .map(|tag| scan_strategies(&data_dir.join("releases").join(tag)))
            .unwrap_or_default();
        let selected_config = available_configs.first().cloned();
        let (tx, rx) = mpsc::channel();

        let mut app = Self {
            data_dir,
            active_tag,
            best_config: None,
            available_configs,
            selected_config,
            active_config: None,
            startup_enabled: startup_is_enabled(),
            is_running: bypass_is_running(),
            last_probe: Instant::now(),
            config_filter: String::new(),
            busy: false,
            status: "Готово к работе".to_owned(),
            tx,
            rx,
            #[cfg(windows)]
            tray: None,
            #[cfg(windows)]
            tray_initialized: false,
            #[cfg(windows)]
            tray_actions: HashMap::new(),
            #[cfg(windows)]
            allow_exit: false,
        };
        app.update_bundle();
        app
    }

    fn active_dir(&self) -> Option<PathBuf> {
        self.active_tag
            .as_ref()
            .map(|tag| self.data_dir.join("releases").join(tag))
    }

    fn check_messages(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            self.busy = false;
            match message {
                BackgroundResult::Installed(Ok((tag, _))) => {
                    self.active_tag = Some(tag.clone());
                    self.available_configs = self
                        .active_dir()
                        .map(|directory| scan_strategies(&directory))
                        .unwrap_or_default();
                    if !self
                        .selected_config
                        .as_ref()
                        .is_some_and(|config| self.available_configs.contains(config))
                    {
                        self.selected_config = self.available_configs.first().cloned();
                    }
                    self.status = format!("Установлена версия {tag}");
                    #[cfg(windows)]
                    self.refresh_tray_menu();
                }
                BackgroundResult::Installed(Err(error)) => {
                    self.status = format!("Ошибка обновления: {error}");
                }
                BackgroundResult::Tested(Ok(Some(config))) => {
                    self.status = format!("Подходящая стратегия: {config}");
                    self.best_config = Some(config.clone());
                    self.selected_config = Some(config);
                    #[cfg(windows)]
                    self.refresh_tray_menu();
                }
                BackgroundResult::Tested(Ok(None)) => {
                    self.status = "Тест завершён, подходящая стратегия не найдена".to_owned();
                }
                BackgroundResult::Tested(Err(error)) => {
                    self.status = format!("Ошибка тестирования: {error}");
                }
            }
        }
    }

    fn update_bundle(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = "Проверяем официальный релиз GitHub…".to_owned();
        let root = self.data_dir.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = install_latest_release(&root);
            let _ = tx.send(BackgroundResult::Installed(result));
        });
    }

    fn test_configurations(&mut self) {
        if self.busy {
            return;
        }
        let Some(release_dir) = self.active_dir() else {
            self.status = "Сначала загрузите официальный релиз".to_owned();
            return;
        };
        self.busy = true;
        self.status = "Откроется окно штатного тестера Flowseal…".to_owned();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = run_upstream_tests(&release_dir);
            let _ = tx.send(BackgroundResult::Tested(result));
        });
    }

    fn launch_best(&mut self) {
        let Some(config) = self.selected_config.clone() else {
            self.status = "В релизе не найдены стратегии".to_owned();
            return;
        };
        let Some(release_dir) = self.active_dir() else {
            self.status = "Сначала загрузите официальный релиз".to_owned();
            return;
        };
        match switch_strategy(&release_dir, &config) {
            Ok(()) => {
                self.active_config = Some(config.clone());
                self.is_running = true;
                self.status = format!("Запущена стратегия {config}");
            }
            Err(error) => self.status = format!("Не удалось запустить стратегию: {error}"),
        }
    }

    fn stop_bypass(&mut self) {
        match stop_running_bypass() {
            Ok(()) => {
                self.is_running = false;
                self.active_config = None;
                self.status = "Обход остановлен".to_owned();
            }
            Err(error) => self.status = format!("Не удалось остановить обход: {error}"),
        }
    }

    fn select_config(&mut self, config: String, apply: bool) {
        if !self.available_configs.contains(&config) {
            return;
        }
        self.selected_config = Some(config);
        #[cfg(windows)]
        self.refresh_tray_menu();
        if apply {
            self.launch_best();
        }
    }

    #[cfg(windows)]
    fn initialize_tray(&mut self) {
        self.tray_initialized = true;
        match make_tray_menu(&self.available_configs, self.selected_config.as_deref()) {
            Ok((menu, actions)) => match TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("EasyRussia")
                .with_icon(make_tray_icon())
                .build()
            {
                Ok(tray) => {
                    self.tray = Some(tray);
                    self.tray_actions = actions;
                }
                Err(error) => self.status = format!("Не удалось создать трей: {error}"),
            },
            Err(error) => self.status = format!("Не удалось создать меню трея: {error}"),
        }
    }

    #[cfg(windows)]
    fn refresh_tray_menu(&mut self) {
        let Ok((menu, actions)) = make_tray_menu(&self.available_configs, self.selected_config.as_deref()) else {
            return;
        };
        if let Some(tray) = &self.tray {
            tray.set_menu(Some(Box::new(menu)));
            self.tray_actions = actions;
        }
    }

    #[cfg(windows)]
    fn handle_tray_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let Some(action) = self.tray_actions.get(&event.id).cloned() else {
                continue;
            };
            match action {
                TrayAction::ShowWindow => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayAction::ApplyConfig(config) => self.select_config(config, true),
                TrayAction::Stop => self.stop_bypass(),
                TrayAction::Test => self.test_configurations(),
                TrayAction::Update => self.update_bundle(),
                TrayAction::Service => self.open_service_manager(),
                TrayAction::Quit => {
                    self.allow_exit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn open_service_manager(&mut self) {
        let Some(release_dir) = self.active_dir() else {
            self.status = "Сначала загрузите официальный релиз".to_owned();
            return;
        };
        match open_service_manager(&release_dir) {
            Ok(()) => self.status = "Открыто штатное меню службы Flowseal".to_owned(),
            Err(error) => self.status = format!("Не удалось открыть меню: {error}"),
        }
    }
}

impl eframe::App for EasyRussiaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_visuals(egui::Visuals::dark());
        ctx.style_mut(|style| {
            style.visuals.panel_fill = egui::Color32::from_rgb(18, 19, 21);
            style.visuals.window_fill = egui::Color32::from_rgb(18, 19, 21);
            style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(39, 41, 45);
            style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(57, 60, 65);
            style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(75, 79, 85);
            style.visuals.selection.bg_fill = egui::Color32::from_rgb(75, 79, 85);
            style.spacing.item_spacing = egui::vec2(12.0, 10.0);
        });
        self.check_messages();
        #[cfg(windows)]
        {
            if !self.tray_initialized {
                self.initialize_tray();
            }
            self.handle_tray_events(ctx);
        }
        if self.last_probe.elapsed() >= Duration::from_secs(2) {
            self.is_running = bypass_is_running();
            if !self.is_running {
                self.active_config = None;
            }
            self.last_probe = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_millis(250));

        if ctx.input(|input| input.viewport().close_requested()) {
            #[cfg(windows)]
            if !self.allow_exit {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(18, 19, 21))
                    .inner_margin(egui::Margin::same(24)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(
                        egui::RichText::new("EasyRussia")
                            .size(24.0)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let version = self
                            .active_tag
                            .as_deref()
                            .map(|tag| format!("Flowseal {tag}"))
                            .unwrap_or_else(|| "Ожидание релиза".to_owned());
                        ui.label(egui::RichText::new(version).color(egui::Color32::GRAY));
                    });
                });
                ui.label(egui::RichText::new("Flowseal · zapret-discord-youtube").color(egui::Color32::GRAY));
                ui.add_space(12.0);
                ui.separator();

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("СОСТОЯНИЕ").size(11.0).strong().color(egui::Color32::GRAY));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (label, color) = if self.is_running {
                            ("●  РАБОТАЕТ", egui::Color32::from_rgb(130, 205, 155))
                        } else {
                            ("○  ОСТАНОВЛЕН", egui::Color32::from_rgb(175, 177, 182))
                        };
                        ui.label(egui::RichText::new(label).strong().color(color));
                    });
                });
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        self.active_config
                            .as_deref()
                            .or(self.selected_config.as_deref())
                            .unwrap_or("Стратегия не выбрана"),
                    )
                    .size(19.0)
                    .strong(),
                );
                ui.add_space(12.0);

                ui.horizontal_wrapped(|ui| {
                    let apply_label = if self.is_running {
                        "Применить профиль"
                    } else {
                        "Запустить профиль"
                    };
                    if ui
                        .add_enabled(
                            !self.busy && self.selected_config.is_some(),
                            egui::Button::new(apply_label),
                        )
                        .clicked()
                    {
                        self.launch_best();
                    }
                    if ui
                        .add_enabled(self.is_running && !self.busy, egui::Button::new("Остановить"))
                        .clicked()
                    {
                        self.stop_bypass();
                    }
                });

                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    ui.heading("Стратегии");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(format!("{}", self.available_configs.len()));
                    });
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.config_filter)
                        .hint_text("Фильтр по названию")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(4.0);
                let filter = self.config_filter.to_lowercase();
                let visible_configs = self
                    .available_configs
                    .iter()
                    .filter(|config| filter.is_empty() || config.to_lowercase().contains(&filter))
                    .cloned()
                    .collect::<Vec<_>>();
                let mut selected_changed = false;
                egui::ScrollArea::vertical()
                    .max_height(190.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for config in &visible_configs {
                            let selected = self.selected_config.as_deref() == Some(config);
                            if ui.selectable_label(selected, config.as_str()).clicked() {
                                self.selected_config = Some(config.clone());
                                selected_changed = true;
                            }
                        }
                        if self.available_configs.is_empty() {
                            ui.label(egui::RichText::new("Загрузка стратегий…").color(egui::Color32::GRAY));
                        } else if visible_configs.is_empty() {
                            ui.label(egui::RichText::new("Ничего не найдено").color(egui::Color32::GRAY));
                        }
                    });
                if selected_changed {
                    #[cfg(windows)]
                    self.refresh_tray_menu();
                }

                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(!self.busy, egui::Button::new(if self.busy { "Выполняется…" } else { "Обновить" }))
                        .clicked()
                    {
                        self.update_bundle();
                    }
                    if ui
                        .add_enabled(!self.busy && self.active_tag.is_some(), egui::Button::new("Тест стратегий"))
                        .clicked()
                    {
                        self.test_configurations();
                    }
                    if ui
                        .add_enabled(!self.busy && self.active_tag.is_some(), egui::Button::new("Служба…"))
                        .clicked()
                    {
                        self.open_service_manager();
                    }
                });

                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(self.status.clone())
                        .color(egui::Color32::from_rgb(190, 192, 196)),
                );
                if let Some(config) = &self.best_config {
                    ui.label(
                        egui::RichText::new(format!("Лучший результат проверки: {config}"))
                            .size(12.0)
                            .color(egui::Color32::GRAY),
                    );
                }

                ui.add_space(12.0);
                ui.separator();
                let mut startup = self.startup_enabled;
                if ui
                    .checkbox(&mut startup, "Запускать приложение при входе в Windows")
                    .changed()
                {
                    match set_startup(startup) {
                        Ok(()) => {
                            self.startup_enabled = startup;
                            self.status = if startup {
                                "Автозапуск приложения включён".to_owned()
                            } else {
                                "Автозапуск приложения выключен".to_owned()
                            };
                        }
                        Err(error) => self.status = format!("Не удалось изменить автозапуск: {error}"),
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !self.busy && self.active_tag.is_some(),
                            egui::Button::new("Открыть настройки службы Flowseal…"),
                        )
                        .clicked()
                    {
                        self.open_service_manager();
                    }
                });
                ui.label(egui::RichText::new(format!("Конфиги: {REPOSITORY}")).size(10.0).color(egui::Color32::GRAY));
            });
    }
}

fn install_latest_release(data_dir: &Path) -> Result<(String, PathBuf), String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("EasyRussia/0.1.0")
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(display_error)?;
    let release: GitHubRelease = client
        .get(RELEASES_URL)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(display_error)?
        .json()
        .map_err(display_error)?;
    let tag = release.tag_name;
    if !valid_tag(&tag) {
        return Err("GitHub вернул некорректный номер релиза".to_owned());
    }
    let releases_dir = data_dir.join("releases");
    let destination = releases_dir.join(&tag);
    if destination.exists() {
        if destination.join("service.bat").is_file() {
            write_active_tag(data_dir, &tag)?;
            return Ok((tag, destination));
        }
        return Err("Папка этой версии уже существует, но выглядит неполной".to_owned());
    }
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name.ends_with(".zip"))
        .ok_or_else(|| "В последнем релизе не найден ZIP-архив".to_owned())?;
    if asset.size == 0 || asset.size > MAX_ARCHIVE_BYTES {
        return Err("Размер архива выходит за допустимые пределы".to_owned());
    }
    let download_url = reqwest::Url::parse(&asset.browser_download_url).map_err(display_error)?;
    if download_url.scheme() != "https"
        || download_url.host_str() != Some("github.com")
        || !download_url
            .path()
            .starts_with("/Flowseal/zapret-discord-youtube/releases/download/")
    {
        return Err("GitHub вернул неожиданный адрес архива".to_owned());
    }

    let response = client
        .get(download_url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(display_error)?;
    if response.content_length().is_some_and(|length| length != asset.size) {
        return Err("Размер архива не совпадает с данными GitHub".to_owned());
    }
    let mut bytes = Vec::with_capacity(asset.size as usize);
    response
        .take(MAX_ARCHIVE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(display_error)?;
    if bytes.len() as u64 != asset.size {
        return Err("Размер скачанного архива не совпадает с данными GitHub".to_owned());
    }

    fs::create_dir_all(&releases_dir).map_err(display_error)?;

    let stage = tempfile::Builder::new()
        .prefix("download-")
        .tempdir_in(&releases_dir)
        .map_err(display_error)?;
    let mut archive = zip::ZipArchive::new(io::Cursor::new(bytes)).map_err(display_error)?;
    let mut unpacked_size = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(display_error)?;
        let Some(relative_path) = entry.enclosed_name().as_deref().map(Path::to_path_buf) else {
            return Err("В архиве обнаружен небезопасный путь".to_owned());
        };
        unpacked_size = unpacked_size.saturating_add(entry.size());
        if unpacked_size > MAX_EXTRACTED_BYTES {
            return Err("Распакованный архив превышает допустимый размер".to_owned());
        }
        let output = stage.path().join(relative_path);
        if entry.is_dir() {
            fs::create_dir_all(&output).map_err(display_error)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(display_error)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(display_error)?;
        io::copy(&mut entry, &mut file).map_err(display_error)?;
    }

    let bundle_dir = find_bundle_dir(stage.path(), 0)
        .ok_or_else(|| "В архиве не найдены service.bat и utils/test zapret.ps1".to_owned())?;
    fs::rename(&bundle_dir, &destination).map_err(display_error)?;
    write_active_tag(data_dir, &tag)?;
    Ok((tag, destination))
}

fn find_bundle_dir(directory: &Path, depth: usize) -> Option<PathBuf> {
    if depth > 3 {
        return None;
    }
    if directory.join("service.bat").is_file()
        && directory.join("utils").join("test zapret.ps1").is_file()
    {
        return Some(directory.to_path_buf());
    }
    for entry in fs::read_dir(directory).ok()? {
        let path = entry.ok()?.path();
        if path.is_dir() {
            if let Some(found) = find_bundle_dir(&path, depth + 1) {
                return Some(found);
            }
        }
    }
    None
}

fn run_upstream_tests(release_dir: &Path) -> Result<Option<String>, String> {
    let script = release_dir.join("utils").join("test zapret.ps1");
    if !script.is_file() {
        return Err("В релизе не найден штатный тестер".to_owned());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x00000010;
        let started_at = SystemTime::now();
        let mut child = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .current_dir(release_dir)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map_err(display_error)?;
        child.wait().map_err(display_error)?;
        return latest_best_config(release_dir, started_at);
    }
    #[cfg(not(windows))]
    {
        let _ = (release_dir, script);
        Err("Запуск тестера поддерживается только в Windows".to_owned())
    }
}

fn latest_best_config(
    release_dir: &Path,
    started_at: SystemTime,
) -> Result<Option<String>, String> {
    let results_dir = release_dir.join("utils").join("test results");
    let mut files = fs::read_dir(results_dir)
        .map_err(display_error)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_file()).unwrap_or(false))
        .collect::<Vec<_>>();
    files.sort_by_key(|entry| {
        entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    let Some(result_file) = files.into_iter().rev().find(|entry| {
        entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified >= started_at)
    }) else {
        return Ok(None);
    };
    let mut text = String::new();
    File::open(result_file.path())
        .and_then(|mut file| file.read_to_string(&mut text))
        .map_err(display_error)?;
    Ok(text.lines().find_map(|line| {
        line.strip_prefix("Best strategy:")
            .map(str::trim)
            .filter(|config| !config.is_empty() && *config != "None")
            .map(str::to_owned)
    }))
}

fn launch_strategy(release_dir: &Path, config: &str) -> Result<(), String> {
    let args = strategy_arguments(release_dir, config)?;
    let bin_dir = release_dir.join("bin");
    let winws = bin_dir.join("winws.exe");
    if !winws.is_file() {
        return Err("В релизе не найден bin/winws.exe".to_owned());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        std::process::Command::new(winws)
            .args(args)
            .current_dir(bin_dir)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(display_error)?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (release_dir, args, winws);
        Err("Запуск стратегий поддерживается только в Windows".to_owned())
    }
}

fn strategy_arguments(release_dir: &Path, config: &str) -> Result<Vec<String>, String> {
    if !valid_strategy_name(config) {
        return Err("Имя стратегии некорректно".to_owned());
    }
    ensure_user_lists(release_dir)?;
    let strategy = release_dir.join(config);
    let batch = fs::read_to_string(&strategy).map_err(display_error)?;
    let mut launch_line = String::new();
    let mut capturing = false;

    for line in batch.lines() {
        let trimmed = line.trim_end();
        if !capturing {
            let lowercase = trimmed.to_ascii_lowercase();
            if !lowercase.trim_start().starts_with("start ") || !lowercase.contains("winws.exe") {
                continue;
            }
            capturing = true;
        }

        let continued = trimmed.ends_with('^');
        let fragment = if continued {
            &trimmed[..trimmed.len() - 1]
        } else {
            trimmed
        };
        if !launch_line.is_empty() {
            launch_line.push(' ');
        }
        launch_line.push_str(fragment.trim());
        if !continued {
            break;
        }
    }

    if !capturing {
        return Err("В стратегии не найдена команда запуска winws.exe".to_owned());
    }
    let lowercase = launch_line.to_ascii_lowercase();
    let executable_end = lowercase
        .find("winws.exe")
        .map(|index| index + "winws.exe".len())
        .ok_or_else(|| "Не удалось разобрать путь к winws.exe".to_owned())?;
    let tail = launch_line[executable_end..].trim_start();
    let arguments = tail
        .strip_prefix('"')
        .unwrap_or(tail)
        .trim_start();
    let game_filter = read_game_port_filter(release_dir);
    let bin_dir = path_prefix(&release_dir.join("bin"));
    let lists_dir = path_prefix(&release_dir.join("lists"));
    let root_dir = path_prefix(release_dir);
    let expanded = expand_batch_variables(
        arguments,
        &[
            ("BIN", bin_dir.as_str()),
            ("LISTS", lists_dir.as_str()),
            ("GAMEFILTERTCP", game_filter.tcp.as_str()),
            ("GAMEFILTERUDP", game_filter.udp.as_str()),
            ("~DP0", root_dir.as_str()),
        ],
    )?;
    split_windows_arguments(&expanded)
}

fn path_prefix(path: &Path) -> String {
    let mut value = path.to_string_lossy().into_owned();
    if !value.ends_with(std::path::MAIN_SEPARATOR) {
        value.push(std::path::MAIN_SEPARATOR);
    }
    value
}

fn expand_batch_variables(input: &str, values: &[(&str, &str)]) -> Result<String, String> {
    let mut expanded = String::with_capacity(input.len());
    let mut remaining = input;
    while let Some(open) = remaining.find('%') {
        expanded.push_str(&remaining[..open]);
        let variable_start = open + 1;
        let Some(close_relative) = remaining[variable_start..].find('%') else {
            return Err("В команде стратегии найдена незакрытая переменная".to_owned());
        };
        let close = variable_start + close_relative;
        let variable = &remaining[variable_start..close];
        let replacement = values
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(variable))
            .map(|(_, value)| *value)
            .ok_or_else(|| format!("Неизвестная переменная в стратегии: %{variable}%"))?;
        expanded.push_str(replacement);
        remaining = &remaining[close + 1..];
    }
    expanded.push_str(remaining);
    Ok(expanded)
}

fn split_windows_arguments(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in input.chars() {
        match character {
            '"' => quoted = !quoted,
            character if character.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if quoted {
        return Err("Кавычки в команде стратегии не сбалансированы".to_owned());
    }
    if !current.is_empty() {
        args.push(current);
    }
    if args.is_empty() {
        return Err("В команде стратегии нет параметров для winws.exe".to_owned());
    }
    Ok(args)
}

#[derive(Debug, PartialEq, Eq)]
struct GamePortFilter {
    tcp: String,
    udp: String,
}

fn read_game_port_filter(release_dir: &Path) -> GamePortFilter {
    let settings = fs::read_to_string(release_dir.join("utils").join("game_filter.enabled"))
        .unwrap_or_default();
    let mut mode = "disabled";
    let mut tcp_candidate = None;
    let mut udp_candidate = None;
    for line in settings.lines() {
        let (key, value) = line.split_once('=').unwrap_or((line, ""));
        match key.trim().to_ascii_lowercase().as_str() {
            "mode" => mode = value.trim(),
            "all" => mode = "all",
            "tcp" if value.trim().is_empty() => mode = "tcp",
            "tcp" => tcp_candidate = Some(value.trim()),
            "udp" if value.trim().is_empty() => mode = "udp",
            "udp" => udp_candidate = Some(value.trim()),
            _ => {}
        }
    }
    let tcp_range = tcp_candidate
        .and_then(validate_port_ranges)
        .unwrap_or_else(|| "1024-65535".to_owned());
    let udp_range = udp_candidate
        .and_then(validate_port_ranges)
        .unwrap_or_else(|| "1024-65535".to_owned());
    match mode.to_ascii_lowercase().as_str() {
        "all" => GamePortFilter {
            tcp: tcp_range,
            udp: udp_range,
        },
        "tcp" => GamePortFilter {
            tcp: tcp_range,
            udp: "12".to_owned(),
        },
        "udp" => GamePortFilter {
            tcp: "12".to_owned(),
            udp: udp_range,
        },
        _ => GamePortFilter {
            tcp: "12".to_owned(),
            udp: "12".to_owned(),
        },
    }
}

fn validate_port_ranges(input: &str) -> Option<String> {
    let normalized = input.chars().filter(|character| !character.is_whitespace()).collect::<String>();
    if normalized.is_empty() {
        return None;
    }
    for range in normalized.split(',') {
        let (start, end) = range.split_once('-').unwrap_or((range, range));
        let start = start.parse::<u32>().ok()?;
        let end = end.parse::<u32>().ok()?;
        if start == 0 || end > 65535 || start > end {
            return None;
        }
    }
    Some(normalized)
}

fn ensure_user_lists(release_dir: &Path) -> Result<(), String> {
    let lists_dir = release_dir.join("lists");
    fs::create_dir_all(&lists_dir).map_err(display_error)?;
    for (name, contents) in [
        ("ipset-exclude-user.txt", "203.0.113.113/32\n"),
        (
            "list-general-user.txt",
            "# Never leave this file empty\ndomain.example.abc\n",
        ),
        ("list-exclude-user.txt", "domain.example.abc\n"),
    ] {
        let path = lists_dir.join(name);
        if !path.exists() {
            fs::write(path, contents).map_err(display_error)?;
        }
    }
    Ok(())
}

fn scan_strategies(release_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(release_dir) else {
        return Vec::new();
    };
    let mut strategies = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_file()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            name.to_ascii_lowercase().starts_with("general") && valid_strategy_name(name)
        })
        .collect::<Vec<_>>();
    strategies.sort_by_key(|name| name.to_ascii_lowercase());
    strategies
}

fn switch_strategy(release_dir: &Path, config: &str) -> Result<(), String> {
    if !valid_strategy_name(config) || !release_dir.join(config).is_file() {
        return Err("Файл стратегии не найден".to_owned());
    }
    #[cfg(windows)]
    {
        if service_is_running() {
            return Err("Служба zapret запущена. Переключите стратегию в меню «Служба…».".to_owned());
        }
        stop_running_bypass()?;
        launch_strategy(release_dir, config)
    }
    #[cfg(not(windows))]
    {
        let _ = release_dir;
        Err("Переключение стратегий поддерживается только в Windows".to_owned())
    }
}

fn bypass_is_running() -> bool {
    #[cfg(windows)]
    {
        if service_is_running() {
            return true;
        }
        return hidden_command_output("tasklist.exe", &["/FI", "IMAGENAME eq winws.exe", "/NH"])
            .map(|output| String::from_utf8_lossy(&output.stdout).contains("winws.exe"))
            .unwrap_or(false);
    }
    #[cfg(not(windows))]
    false
}

fn service_is_running() -> bool {
    #[cfg(windows)]
    {
        return hidden_command_output("sc.exe", &["query", "zapret"])
            .map(|output| String::from_utf8_lossy(&output.stdout).contains("RUNNING"))
            .unwrap_or(false);
    }
    #[cfg(not(windows))]
    false
}

fn stop_running_bypass() -> Result<(), String> {
    #[cfg(windows)]
    {
        if service_is_running() {
            let output = hidden_command_output("sc.exe", &["stop", "zapret"])
                .map_err(display_error)?;
            if !output.status.success() && service_is_running() {
                return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
            }
            return Ok(());
        }
        let output = hidden_command_output("tasklist.exe", &["/FI", "IMAGENAME eq winws.exe", "/NH"])
            .map_err(display_error)?;
        if !String::from_utf8_lossy(&output.stdout).contains("winws.exe") {
            return Ok(());
        }
        let output = hidden_command_output("taskkill.exe", &["/IM", "winws.exe", "/F"])
            .map_err(display_error)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    }
    #[cfg(not(windows))]
    Err("Остановка поддерживается только в Windows".to_owned())
}

#[cfg(windows)]
fn hidden_command_output(
    program: &str,
    args: &[&str],
) -> io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

#[cfg(windows)]
fn make_tray_menu(
    configs: &[String],
    selected_config: Option<&str>,
) -> Result<(Menu, HashMap<MenuId, TrayAction>), String> {
    let menu = Menu::new();
    let mut actions = HashMap::new();
    append_tray_item(&menu, &mut actions, "show", "Открыть EasyRussia", TrayAction::ShowWindow)?;
    menu.append(&PredefinedMenuItem::separator()).map_err(display_error)?;

    let profiles = Submenu::new("Стратегии", !configs.is_empty());
    for (index, config) in configs.iter().enumerate() {
        let id = format!("profile-{index}");
        let title = if selected_config == Some(config.as_str()) {
            format!("✓  {config}")
        } else {
            config.clone()
        };
        let item = MenuItem::with_id(id, title, true, None);
        actions.insert(item.id().clone(), TrayAction::ApplyConfig(config.clone()));
        profiles.append(&item).map_err(display_error)?;
    }
    menu.append(&profiles).map_err(display_error)?;
    menu.append(&PredefinedMenuItem::separator()).map_err(display_error)?;
    append_tray_item(&menu, &mut actions, "stop", "Остановить обход", TrayAction::Stop)?;
    append_tray_item(&menu, &mut actions, "test", "Проверить стратегии", TrayAction::Test)?;
    append_tray_item(&menu, &mut actions, "update", "Проверить обновления", TrayAction::Update)?;
    append_tray_item(&menu, &mut actions, "service", "Настройки службы…", TrayAction::Service)?;
    menu.append(&PredefinedMenuItem::separator()).map_err(display_error)?;
    append_tray_item(&menu, &mut actions, "quit", "Выход", TrayAction::Quit)?;
    Ok((menu, actions))
}

#[cfg(windows)]
fn append_tray_item(
    menu: &Menu,
    actions: &mut HashMap<MenuId, TrayAction>,
    id: &str,
    title: &str,
    action: TrayAction,
) -> Result<(), String> {
    let item = MenuItem::with_id(id, title, true, None);
    actions.insert(item.id().clone(), action);
    menu.append(&item).map_err(display_error)
}

#[cfg(windows)]
fn make_tray_icon() -> tray_icon::Icon {
    let mut rgba = vec![0_u8; 32 * 32 * 4];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[24, 25, 28, 255]);
    }
    for y in 8..24 {
        for x in 8..12 {
            set_icon_pixel(&mut rgba, x, y);
        }
    }
    for y in [8..12, 14..18, 20..24] {
        for row in y {
            for x in 8..24 {
                set_icon_pixel(&mut rgba, x, row);
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, 32, 32).expect("valid 32x32 tray icon")
}

#[cfg(windows)]
fn set_icon_pixel(rgba: &mut [u8], x: usize, y: usize) {
    let offset = (y * 32 + x) * 4;
    rgba[offset..offset + 4].copy_from_slice(&[245, 245, 245, 255]);
}

fn open_service_manager(release_dir: &Path) -> Result<(), String> {
    let script = release_dir.join("service.bat");
    if !script.is_file() {
        return Err("В релизе не найден service.bat".to_owned());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x00000010;
        std::process::Command::new("cmd.exe")
            .arg("/C")
            .arg(script)
            .current_dir(release_dir)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map_err(display_error)?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (release_dir, script);
        Err("Менеджер службы поддерживается только в Windows".to_owned())
    }
}

fn read_active_tag(data_dir: &Path) -> Option<String> {
    let tag = fs::read_to_string(data_dir.join("active_release")).ok()?;
    let tag = tag.trim();
    if valid_tag(tag) && data_dir.join("releases").join(tag).join("service.bat").is_file() {
        Some(tag.to_owned())
    } else {
        None
    }
}

fn write_active_tag(data_dir: &Path, tag: &str) -> Result<(), String> {
    fs::create_dir_all(data_dir).map_err(display_error)?;
    fs::write(data_dir.join("active_release"), tag).map_err(display_error)
}

fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag != "."
        && tag != ".."
        && tag.len() <= 80
        && tag
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_'))
}

fn valid_strategy_name(config: &str) -> bool {
    !config.is_empty()
        && !config.contains('/')
        && !config.contains('\\')
        && Path::new(config).file_name().and_then(|name| name.to_str()) == Some(config)
        && config.to_ascii_lowercase().ends_with(".bat")
}

fn set_startup(enabled: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let run_key = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
            .map_err(display_error)?
            .0;
        if enabled {
            let executable = std::env::current_exe().map_err(display_error)?;
            let command = format!("\"{}\"", executable.display());
            run_key.set_value("EasyRussia", &command).map_err(display_error)?;
        } else {
            let _ = run_key.delete_value("EasyRussia");
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err("Автозапуск поддерживается только в Windows".to_owned())
    }
}

fn startup_is_enabled() -> bool {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let Ok(run_key) = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        else {
            return false;
        };
        return run_key.get_value::<String, _>("EasyRussia").is_ok();
    }
    #[cfg(not(windows))]
    false
}

fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EasyRussia")
            .with_inner_size([780.0, 620.0])
            .with_min_inner_size([640.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "EasyRussia",
        options,
        Box::new(|_creation_context| Ok(Box::new(EasyRussiaApp::new()))),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        latest_best_config, read_game_port_filter, scan_strategies, strategy_arguments,
        validate_port_ranges, valid_strategy_name, valid_tag, GamePortFilter,
    };
    use std::fs;
    use std::time::{Duration, SystemTime};

    #[test]
    fn release_tags_reject_paths_and_invalid_characters() {
        assert!(valid_tag("1.10.4"));
        assert!(valid_tag("v1.2-rc_1"));
        assert!(!valid_tag(""));
        assert!(!valid_tag(".."));
        assert!(!valid_tag("../outside"));
        assert!(!valid_tag("release/tag"));
    }

    #[test]
    fn strategy_names_must_be_bat_files_without_path_components() {
        assert!(valid_strategy_name("general (ALT).bat"));
        assert!(!valid_strategy_name("../general.bat"));
        assert!(!valid_strategy_name("..\\general.bat"));
        assert!(!valid_strategy_name("service.bat.exe"));
        assert!(!valid_strategy_name(""));
    }

    #[test]
    fn strategy_scan_returns_sorted_general_bat_files_only() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(directory.path().join("general (ALT).bat"), "").expect("strategy file");
        fs::write(directory.path().join("general.bat"), "").expect("strategy file");
        fs::write(directory.path().join("service.bat"), "").expect("service file");
        fs::write(directory.path().join("general.txt"), "").expect("non-strategy file");

        assert_eq!(
            scan_strategies(directory.path()),
            vec!["general (ALT).bat", "general.bat"]
        );
    }

    #[test]
    fn strategy_arguments_expand_paths_filters_and_continuations() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let utils = directory.path().join("utils");
        fs::create_dir_all(&utils).expect("utils directory");
        fs::create_dir_all(directory.path().join("bin")).expect("bin directory");
        fs::create_dir_all(directory.path().join("lists")).expect("lists directory");
        fs::write(
            utils.join("game_filter.enabled"),
            "mode=all\ntcp=443,8443-8444\nudp=53\n",
        )
        .expect("game filter settings");
        fs::write(
            directory.path().join("general.bat"),
            "@echo off\nstart \"zapret\" /min \"%BIN%winws.exe\" --wf-tcp=80,%GameFilterTCP% --wf-udp=%GameFilterUDP% ^\n--list=\"%LISTS%custom list.txt\" --fake=\"%BIN%active.bin\"\n",
        )
        .expect("strategy file");

        let args = strategy_arguments(directory.path(), "general.bat")
            .expect("parse strategy arguments");
        let expected_list = format!(
            "--list={}custom list.txt",
            super::path_prefix(&directory.path().join("lists"))
        );
        let expected_fake = format!(
            "--fake={}active.bin",
            super::path_prefix(&directory.path().join("bin"))
        );
        assert_eq!(
            args,
            vec![
                "--wf-tcp=80,443,8443-8444".to_owned(),
                "--wf-udp=53".to_owned(),
                expected_list,
                expected_fake,
            ]
        );
        assert!(directory.path().join("lists/list-general-user.txt").is_file());
    }

    #[test]
    fn game_filter_defaults_to_disabled_and_rejects_bad_port_ranges() {
        let directory = tempfile::tempdir().expect("temporary directory");
        assert_eq!(
            read_game_port_filter(directory.path()),
            GamePortFilter {
                tcp: "12".to_owned(),
                udp: "12".to_owned(),
            }
        );
        assert_eq!(validate_port_ranges("53,1000-2000"), Some("53,1000-2000".to_owned()));
        assert_eq!(validate_port_ranges("0,70000"), None);
        assert_eq!(validate_port_ranges("2000-1000"), None);
    }

    #[test]
    fn reads_best_strategy_from_upstream_report() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let results = directory.path().join("utils").join("test results");
        fs::create_dir_all(&results).expect("results directory");
        fs::write(
            results.join("test_results.txt"),
            "=== ANALYTICS ===\nBest strategy: general (ALT).bat\n",
        )
        .expect("result report");

        let config = latest_best_config(directory.path(), SystemTime::UNIX_EPOCH)
            .expect("parse result report");
        assert_eq!(config.as_deref(), Some("general (ALT).bat"));
    }

    #[test]
    fn ignores_reports_older_than_the_current_test_run() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let results = directory.path().join("utils").join("test results");
        fs::create_dir_all(&results).expect("results directory");
        fs::write(
            results.join("test_results.txt"),
            "Best strategy: general (ALT).bat\n",
        )
        .expect("result report");

        let started_at = SystemTime::now() + Duration::from_secs(60);
        let config = latest_best_config(directory.path(), started_at).expect("parse results");
        assert_eq!(config, None);
    }
}