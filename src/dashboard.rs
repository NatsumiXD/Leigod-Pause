use crate::{
    history::History,
    report,
    settings::Settings,
    terminal::{InputEvent, Terminal},
};
use serde_json::Value;
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};

const HOME_ITEMS: [&str; 3] = ["历史日志 / 导出", "配置", "退出启动器并关闭雷神"];
const LOG_ITEMS: [&str; 4] = ["返回主页", "配置", "导出历史日志", "退出启动器并关闭雷神"];
#[derive(Clone, Copy, Debug, PartialEq)]
enum Field {
    Delay,
    Enabled,
}
#[derive(Clone, Debug, PartialEq)]
enum Page {
    Home,
    Log,
    Config {
        draft: Settings,
        editing: Option<Field>,
    },
}
#[derive(Debug, PartialEq)]
enum Action {
    None,
    OpenLog,
    ExportLog,
    Save(Settings),
    Quit,
}
struct Menu {
    page: Page,
    home_selected: usize,
    log_selected: usize,
    config_selected: usize,
    notice: String,
}
impl Default for Menu {
    fn default() -> Self {
        Self {
            page: Page::Home,
            home_selected: 0,
            log_selected: 0,
            config_selected: 0,
            notice: String::new(),
        }
    }
}
fn shift(index: usize, count: usize, delta: isize) -> usize {
    (index as isize + delta).rem_euclid(count as isize) as usize
}
impl Menu {
    fn home(&mut self) {
        self.page = Page::Home;
        self.notice.clear();
    }
    fn config(&mut self, settings: &Settings) {
        self.page = Page::Config {
            draft: settings.clone(),
            editing: None,
        };
        self.config_selected = 0;
        self.notice = "Enter 编辑时间或切换开关，←/→ 微调分钟数。".into();
    }
    fn handle(&mut self, event: InputEvent, settings: &Settings) -> Action {
        match event {
            InputEvent::Quit => return Action::Quit,
            InputEvent::Escape => {
                if let Page::Config {
                    editing: Some(_), ..
                } = &self.page
                {
                    if let Page::Config { editing, .. } = &mut self.page {
                        *editing = None;
                    }
                    self.notice = "已取消本次输入；尚未保存的配置仍可修改。".into();
                } else {
                    self.home();
                }
            }
            InputEvent::Up | InputEvent::Down => {
                let delta = if event == InputEvent::Up { -1 } else { 1 };
                match &self.page {
                    Page::Home => {
                        self.home_selected = shift(self.home_selected, HOME_ITEMS.len(), delta)
                    }
                    Page::Log => {
                        self.log_selected = shift(self.log_selected, LOG_ITEMS.len(), delta)
                    }
                    Page::Config { editing: None, .. } => {
                        self.config_selected = shift(self.config_selected, 4, delta)
                    }
                    _ => {}
                }
            }
            InputEvent::Left | InputEvent::Right => {
                if let Page::Config {
                    draft,
                    editing: None,
                } = &mut self.page
                    && self.config_selected == 0
                {
                    draft.exit_delay_minutes = if event == InputEvent::Left {
                        draft.exit_delay_minutes.saturating_sub(1)
                    } else {
                        draft.exit_delay_minutes.saturating_add(1).min(1440)
                    };
                    self.notice = "等待时间已修改，选择“保存配置”生效。".into();
                }
            }
            InputEvent::Enter(text) => {
                let text = text.trim();
                match text.to_ascii_lowercase().as_str() {
                    "quit" | "exit" => return Action::Quit,
                    "home" | "back" => {
                        self.home();
                        return Action::None;
                    }
                    "config" => {
                        self.config(settings);
                        return Action::None;
                    }
                    "log" => {
                        return if matches!(self.page, Page::Log) {
                            Action::ExportLog
                        } else {
                            Action::OpenLog
                        };
                    }
                    "cancel" => {
                        self.home();
                        self.notice = "已取消修改".into();
                        return Action::None;
                    }
                    "save" => {
                        if let Page::Config { draft, .. } = &self.page {
                            return Action::Save(draft.clone());
                        }
                    }
                    _ => {}
                }
                if let Page::Config {
                    draft,
                    editing: Some(field),
                } = &mut self.page
                {
                    let valid = match field {
                        Field::Delay => match text.parse::<u64>() {
                            Ok(n) if n <= 1440 => {
                                draft.exit_delay_minutes = n;
                                true
                            }
                            _ => false,
                        },
                        Field::Enabled => match text {
                            "true" | "1" | "yes" | "是" => {
                                draft.auto_stop_enabled = true;
                                true
                            }
                            "false" | "0" | "no" | "否" => {
                                draft.auto_stop_enabled = false;
                                true
                            }
                            _ => false,
                        },
                    };
                    if valid {
                        if let Page::Config { editing, .. } = &mut self.page {
                            *editing = None;
                        }
                        self.notice = "已修改，选择“保存配置”生效。".into();
                    } else {
                        self.notice = if *field == Field::Delay {
                            "请输入 0–1440 的整分钟数，或按 Esc 取消输入。"
                        } else {
                            "请输入 true 或 false，或按 Esc 取消输入。"
                        }
                        .into();
                    }
                    return Action::None;
                }
                if text.is_empty() {
                    match &mut self.page {
                        Page::Home => match self.home_selected {
                            0 => return Action::OpenLog,
                            1 => self.config(settings),
                            _ => return Action::Quit,
                        },
                        Page::Log => match self.log_selected {
                            0 => self.home(),
                            1 => self.config(settings),
                            2 => return Action::ExportLog,
                            _ => return Action::Quit,
                        },
                        Page::Config { draft, editing } => match self.config_selected {
                            0 => {
                                *editing = Some(Field::Delay);
                                self.notice =
                                    "输入新的等待分钟数，Enter 确认，Esc 取消输入。".into();
                            }
                            1 => {
                                draft.auto_stop_enabled = !draft.auto_stop_enabled;
                                self.notice = "开关已修改，选择“保存配置”生效。".into();
                            }
                            2 => return Action::Save(draft.clone()),
                            _ => {
                                self.home();
                                self.notice = "已取消修改".into();
                            }
                        },
                    }
                } else if let Page::Config { editing, .. } = &mut self.page {
                    match text {
                        "1" => *editing = Some(Field::Delay),
                        "2" => *editing = Some(Field::Enabled),
                        _ => {
                            self.notice = "使用 ↑/↓ 选择、Enter 确认，或输入 save / cancel。".into()
                        }
                    }
                } else {
                    self.notice = "使用 ↑/↓ 和 Enter，或输入 home / log / config / quit。".into();
                }
            }
        }
        Action::None
    }
    fn prompt(&self) -> String {
        match &self.page {
            Page::Log => format!("↑↓ Enter | {} | Esc 返回 ", LOG_ITEMS[self.log_selected]),
            Page::Config {
                editing: Some(Field::Delay),
                ..
            } => "等待分钟（0–1440） ".into(),
            Page::Config {
                editing: Some(Field::Enabled),
                ..
            } => "启用自动停止（true / false） ".into(),
            _ => "↑↓ 选择  Enter 确认  Esc 返回 | 命令 ".into(),
        }
    }
}
fn menu_line(selected: bool, label: &str) -> String {
    format!("{} {label}", if selected { ">" } else { " " })
}
pub struct Dashboard {
    terminal: Terminal,
    menu: Menu,
    last_frame: Vec<String>,
    painted: Instant,
}
impl Dashboard {
    pub fn enter() -> io::Result<Self> {
        Ok(Self {
            terminal: Terminal::enter()?,
            menu: Menu::default(),
            last_frame: vec![],
            painted: Instant::now(),
        })
    }
    pub fn poll(
        &mut self,
        settings: &mut Settings,
        path: &Path,
        history: &History,
    ) -> io::Result<bool> {
        for event in self.terminal.poll_events()? {
            self.last_frame.clear();
            match self.menu.handle(event, settings) {
                Action::None => {}
                Action::Quit => return Ok(true),
                Action::OpenLog => {
                    let result = (|| -> io::Result<()> {
                        let export = history.export()?;
                        self.terminal.switch_log(
                            &history.read_all()?,
                            &format!("历史日志（持续追加）| 已导出: {}", export.display()),
                        )?;
                        Ok(())
                    })();
                    match result {
                        Ok(()) => {
                            self.menu.page = Page::Log;
                            self.menu.log_selected = 0;
                            self.menu.notice.clear();
                        }
                        Err(e) => {
                            self.menu.home();
                            self.menu.notice = format!("日志打开失败: {e}");
                        }
                    }
                }
                Action::ExportLog => match history.export() {
                    Ok(export) => self
                        .terminal
                        .append_log(&format!("日志已导出: {}\n", export.display()))?,
                    Err(e) => self.terminal.append_log(&format!("导出失败: {e}\n"))?,
                },
                Action::Save(draft) => match draft.save(path) {
                    Ok(()) => {
                        *settings = draft;
                        self.menu.home();
                        self.menu.notice = "配置已保存".into();
                    }
                    Err(e) => self.menu.notice = format!("保存失败: {e}"),
                },
            }
            self.terminal
                .set_prompt_prefix(&self.menu.prompt(), matches!(self.menu.page, Page::Log))?;
        }
        Ok(false)
    }
    pub fn append(&mut self, block: &str) -> io::Result<()> {
        if matches!(self.menu.page, Page::Log) {
            self.terminal.append_log(block)?;
        }
        Ok(())
    }
    pub fn draw(&mut self, value: &Value) -> io::Result<()> {
        let remaining = value["auto_stop"]["remaining_seconds"].as_u64();
        let detection = value["auto_stop"]["message"].as_str().unwrap_or("等待检测");
        let title = format!(
            "雷神启动器 | {} | {}{}",
            report::label(value),
            detection,
            remaining
                .map(|n| format!(" {:02}:{:02}", n / 60, n % 60))
                .unwrap_or_default()
        );
        self.terminal.set_title(&title)?;
        self.terminal
            .set_prompt_prefix(&self.menu.prompt(), matches!(self.menu.page, Page::Log))?;
        let (mut lines, highlight) = match &self.menu.page {
            Page::Log => return Ok(()),
            Page::Home => {
                let mut lines = vec![format!(
                    "主页 | 每 {} 秒采集 | 第 {} 次",
                    value["sample_interval_ms"].as_u64().unwrap_or(5000) as f64 / 1000.0,
                    value["sample_number"].as_u64().unwrap_or(0)
                )];
                lines.extend(
                    HOME_ITEMS
                        .iter()
                        .enumerate()
                        .map(|(i, label)| menu_line(i == self.menu.home_selected, label)),
                );
                lines.push("────────────────────────────────────────────────────────────".into());
                lines.extend(report::text(value).lines().map(str::to_owned));
                (lines, Some(1 + self.menu.home_selected))
            }
            Page::Config { draft, editing } => {
                let labels = [
                    format!("游戏退出等待时间     [ {} 分钟 ]", draft.exit_delay_minutes),
                    format!(
                        "自动停止并暂停计时   [ {} ]",
                        if draft.auto_stop_enabled {
                            "已启用"
                        } else {
                            "已禁用"
                        }
                    ),
                    "保存配置".into(),
                    "取消并返回".into(),
                ];
                let mut lines = vec!["配置 | 修改后选择“保存配置”生效".into()];
                lines.extend(
                    labels
                        .iter()
                        .enumerate()
                        .map(|(i, label)| menu_line(i == self.menu.config_selected, label)),
                );
                lines.push("↑/↓ 选择  Enter 编辑/切换  ←/→ 调整分钟  Esc 返回".into());
                if editing.is_some() {
                    lines.push("正在输入新值；Enter 确认输入，Esc 取消本次输入。".into());
                }
                (lines, Some(1 + self.menu.config_selected))
            }
        };
        if !self.menu.notice.is_empty() {
            lines.insert(0, self.menu.notice.clone());
        }
        let highlight = highlight.map(|i| i + usize::from(!self.menu.notice.is_empty()));
        if lines != self.last_frame || self.painted.elapsed() >= Duration::from_secs(1) {
            self.terminal.draw(&lines, &title, highlight)?;
            self.last_frame = lines;
            self.painted = Instant::now();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn enter() -> InputEvent {
        InputEvent::Enter(String::new())
    }
    #[test]
    fn arrows_open_config_and_edit_then_save_without_changing_active_settings() {
        let mut m = Menu::default();
        let settings = Settings::default();
        m.handle(InputEvent::Down, &settings);
        assert_eq!(m.handle(enter(), &settings), Action::None);
        assert!(matches!(m.page, Page::Config { .. }));
        m.handle(enter(), &settings);
        m.handle(InputEvent::Enter("3".into()), &settings);
        m.handle(InputEvent::Down, &settings);
        m.handle(enter(), &settings);
        assert_eq!(settings, Settings::default());
        m.handle(InputEvent::Down, &settings);
        assert_eq!(
            m.handle(enter(), &settings),
            Action::Save(Settings {
                exit_delay_minutes: 3,
                auto_stop_enabled: false
            })
        );
    }
    #[test]
    fn escape_cancels_invalid_field_then_discards_the_draft() {
        let mut m = Menu::default();
        let s = Settings::default();
        m.config(&s);
        m.handle(enter(), &s);
        m.handle(InputEvent::Enter("2000".into()), &s);
        assert!(matches!(
            m.page,
            Page::Config {
                editing: Some(Field::Delay),
                ..
            }
        ));
        m.handle(InputEvent::Escape, &s);
        m.handle(InputEvent::Left, &s);
        m.handle(InputEvent::Escape, &s);
        assert_eq!(m.page, Page::Home);
        m.config(&s);
        assert!(matches!(m.page,Page::Config{draft,editing:None}if draft==s));
    }
    #[test]
    fn log_menu_uses_arrows_and_enter_without_reopening_the_log() {
        let mut m = Menu::default();
        let s = Settings::default();
        assert_eq!(m.handle(enter(), &s), Action::OpenLog);
        m.page = Page::Log;
        m.handle(InputEvent::Down, &s);
        m.handle(InputEvent::Down, &s);
        assert_eq!(m.handle(enter(), &s), Action::ExportLog);
        assert_eq!(m.page, Page::Log);
        m.handle(InputEvent::Escape, &s);
        assert_eq!(m.page, Page::Home);
    }
    #[test]
    fn navigation_wraps_and_minute_adjustments_obey_bounds() {
        let mut m = Menu::default();
        let s = Settings::default();
        m.handle(InputEvent::Up, &s);
        assert_eq!(m.handle(enter(), &s), Action::Quit);
        m.config(&Settings {
            exit_delay_minutes: 0,
            ..s.clone()
        });
        m.handle(InputEvent::Left, &s);
        assert!(matches!(&m.page,Page::Config{draft,..}if draft.exit_delay_minutes==0));
        m.config(&Settings {
            exit_delay_minutes: 1440,
            ..s.clone()
        });
        m.handle(InputEvent::Right, &s);
        assert!(matches!(&m.page,Page::Config{draft,..}if draft.exit_delay_minutes==1440));
    }
}
