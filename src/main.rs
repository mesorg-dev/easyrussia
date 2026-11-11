#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use serde::Deserialize;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

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

struct EasyRussiaApp {
    data_dir: PathBuf,
    active_tag: Option<String>,
    best_config: Option<String>,
    startup_enabled: bool,
    busy: bool,
    status: String,
    tx: Sender<BackgroundResult>,
    rx: Receiver<BackgroundResult>,
}

impl EasyRussiaApp {
    fn new() -> Self {
        let data_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("EasyRussia");
        let active_tag = read_active_tag(&data_dir);
        let (tx, rx) = mpsc::channel();

        let mut app = Self {
            data_dir,
            active_tag,
            best_config: None,
            startup_enabled: startup_is_enabled(),
            busy: false,
            status: "Готово к работе".to_owned(),
            tx,
            rx,
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
                    self.status = format!("Установлена версия {tag}");
                }
                BackgroundResult::Installed(Err(error)) => {
                    self.status = format!("Ошибка обновления: {error}");
                }
                BackgroundResult::Tested(Ok(Some(config))) => {
                    self.status = format!("Подходящая стратегия: {config}");
                    self.best_config = Some(config);
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
        let Some(config) = self.best_config.as_deref() else {
            self.status = "Сначала запустите проверку конфигураций".to_owned();
            return;
        };
        let Some(release_dir) = self.active_dir() else {
            self.status = "Сначала загрузите официальный релиз".to_owned();
            return;
        };
        match launch_strategy(&release_dir, config) {
            Ok(()) => self.status = format!("Запущена стратегия {config}"),
            Err(error) => self.status = format!("Не удалось запустить стратегию: {error}"),
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
        ctx.set_visuals(egui::Visuals::light());
        self.check_messages();
        if self.busy {
            ctx.request_repaint_after(Duration::from_millis(150));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::from_rgb(245, 245, 245)))
            .show(ctx, |ui| {
                ui.add_space(10.0);
                egui::Frame::default()
                    .fill(egui::Color32::BLACK)
                    .inner_margin(egui::Margin::same(22))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new("EASYRUSSIA")
                                .size(13.0)
                                .strong()
                                .color(egui::Color32::from_rgb(190, 190, 190)),
                        );
                        ui.add_space(6.0);
                        ui.heading(
                            egui::RichText::new("Сетевой профиль")
                                .size(28.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Windows · Flowseal / zapret-discord-youtube")
                                .color(egui::Color32::from_rgb(190, 190, 190)),
                        );
                    });

                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    ui.heading("Управление");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let version = self
                            .active_tag
                            .as_deref()
                            .map(|tag| format!("Flowseal {tag}"))
                            .unwrap_or_else(|| "Релиз не установлен".to_owned());
                        ui.label(version);
                    });
                });
                ui.separator();
                ui.add_space(8.0);

                ui.horizontal_wrapped(|ui| {
                    let update = ui.add_enabled(!self.busy, egui::Button::new("↓  Скачать / обновить"));
                    if update.clicked() {
                        self.update_bundle();
                    }
                    let test = ui.add_enabled(
                        !self.busy && self.active_tag.is_some(),
                        egui::Button::new("◉  Подобрать стратегию"),
                    );
                    if test.clicked() {
                        self.test_configurations();
                    }
                    let launch = ui.add_enabled(
                        !self.busy && self.best_config.is_some(),
                        egui::Button::new("▶  Запустить лучшую"),
                    );
                    if launch.clicked() {
                        self.launch_best();
                    }
                });

                ui.add_space(16.0);
                egui::Frame::default()
                    .fill(egui::Color32::WHITE)
                    .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_gray(220)))
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("СОСТОЯНИЕ").size(11.0).strong());
                        ui.add_space(5.0);
                        ui.label(&self.status);
                        if let Some(config) = &self.best_config {
                            ui.add_space(8.0);
                            ui.label(format!("Лучший результат теста: {config}"));
                        }
                    });

                ui.add_space(20.0);
                ui.heading("Автозапуск и служба");
                ui.separator();
                let mut startup = self.startup_enabled;
                if ui
                    .checkbox(&mut startup, "Открывать EasyRussia при входе в Windows")
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
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "Автозапуск приложения и системная служба настраиваются отдельно. Обновление файлов не переустанавливает службу; для этого откройте её меню. Windows запросит необходимые права.",
                    )
                    .size(12.0)
                    .color(egui::Color32::DARK_GRAY),
                );

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!(
                            "Конфиги и тестер поставляет {REPOSITORY}. Перед запуском проверяйте содержимое релиза."
                        ))
                        .size(11.0)
                        .color(egui::Color32::DARK_GRAY),
                    );
                });
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

    let mut response = client
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
    if !valid_strategy_name(config) {
        return Err("Имя стратегии в отчёте некорректно".to_owned());
    }
    let strategy = release_dir.join(config);
    if !strategy.is_file() {
        return Err("Файл выбранной стратегии не найден".to_owned());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x00000010;
        std::process::Command::new("cmd.exe")
            .args(["/C", "start", "", "/min"])
            .arg(strategy)
            .current_dir(release_dir)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map_err(display_error)?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (release_dir, strategy);
        Err("Запуск стратегий поддерживается только в Windows".to_owned())
    }
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
    use super::{latest_best_config, valid_strategy_name, valid_tag};
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