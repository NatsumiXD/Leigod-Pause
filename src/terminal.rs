//! Small Win32 terminal layer: an in-place dashboard and an append-only log.
use std::io;
const SELECTION_STYLE: &str = "\x1b[1;38;2;255;255;255;46m";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Up,
    Down,
    Left,
    Right,
    Enter(String),
    Escape,
    Quit,
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

// Conservative widths for ordinary console text, CJK, combining marks and emoji.
// Complex emoji may be overcounted, which clips early rather than wrapping.
fn char_width(c: char) -> usize {
    match c as u32 {
        0x0300..=0x036f
        | 0x1ab0..=0x1aff
        | 0x1dc0..=0x1dff
        | 0x200b..=0x200f
        | 0x20d0..=0x20ff
        | 0xfe00..=0xfe0f
        | 0xfe20..=0xfe2f
        | 0xe0100..=0xe01ef => 0,
        0x1100..=0x115f
        | 0x2329..=0x232a
        | 0x2e80..=0xa4cf
        | 0xac00..=0xd7a3
        | 0xf900..=0xfaff
        | 0xfe10..=0xfe19
        | 0xfe30..=0xfe6f
        | 0xff01..=0xff60
        | 0xffe0..=0xffe6
        | 0x1f000..=0x1faff
        | 0x20000..=0x3fffd => 2,
        _ => 1,
    }
}

fn clip(text: &str, columns: usize) -> String {
    let mut width = 0;
    sanitize(text)
        .chars()
        .take_while(|&c| {
            let next = width + char_width(c);
            if next > columns {
                false
            } else {
                width = next;
                true
            }
        })
        .collect()
}

fn prompt(prefix: &str, input: &str, columns: usize) -> String {
    let clean = sanitize(input);
    let prefix = format!("{}> ", sanitize(prefix));
    let available = columns.saturating_sub(prefix.chars().map(char_width).sum());
    let mut width = 0;
    let tail: Vec<char> = clean
        .chars()
        .rev()
        .take_while(|&c| {
            let next = width + char_width(c);
            if next > available {
                false
            } else {
                width = next;
                true
            }
        })
        .collect();
    clip(
        &format!("{prefix}{}", tail.into_iter().rev().collect::<String>()),
        columns,
    )
}

#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        System::Console::*,
    };

    pub struct Terminal {
        input_handle: HANDLE,
        output_handle: HANDLE,
        original_input: u32,
        original_output: u32,
        original_title: Vec<u16>,
        input: String,
        high_surrogate: Option<u16>,
        alternate: bool,
        current_title: String,
        prompt_prefix: String,
        prompt_highlight: bool,
    }

    impl Terminal {
        pub fn enter() -> io::Result<Self> {
            // SAFETY: the APIs use process-owned standard handles and writable
            // local storage. These borrowed handles must not be closed here.
            unsafe {
                let input_handle = GetStdHandle(STD_INPUT_HANDLE);
                let output_handle = GetStdHandle(STD_OUTPUT_HANDLE);
                if input_handle.is_null()
                    || input_handle == INVALID_HANDLE_VALUE
                    || output_handle.is_null()
                    || output_handle == INVALID_HANDLE_VALUE
                {
                    return Err(io::Error::new(
                        io::ErrorKind::NotConnected,
                        "Console handles unavailable",
                    ));
                }
                let mut original_input = 0;
                let mut original_output = 0;
                if GetConsoleMode(input_handle, &mut original_input) == 0
                    || GetConsoleMode(output_handle, &mut original_output) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let mut original_title = vec![0u16; 32768];
                let title_len =
                    GetConsoleTitleW(original_title.as_mut_ptr(), original_title.len() as u32);
                original_title.truncate(title_len as usize);
                original_title.push(0);
                let raw = (original_input | ENABLE_EXTENDED_FLAGS)
                    & !(ENABLE_LINE_INPUT
                        | ENABLE_ECHO_INPUT
                        | ENABLE_PROCESSED_INPUT
                        | ENABLE_QUICK_EDIT_MODE
                        | ENABLE_VIRTUAL_TERMINAL_INPUT);
                if SetConsoleMode(input_handle, raw) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let vt = original_output
                    | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                    | ENABLE_PROCESSED_OUTPUT
                    | ENABLE_WRAP_AT_EOL_OUTPUT;
                if SetConsoleMode(output_handle, vt) == 0 {
                    let error = io::Error::last_os_error();
                    SetConsoleMode(input_handle, original_input);
                    return Err(error);
                }
                let mut terminal = Self {
                    input_handle,
                    output_handle,
                    original_input,
                    original_output,
                    original_title,
                    input: String::new(),
                    high_surrogate: None,
                    alternate: true,
                    current_title: String::new(),
                    prompt_prefix: String::new(),
                    prompt_highlight: false,
                };
                terminal.write("\x1b[?1049h\x1b[0m\x1b[2J\x1b[H\x1b[?25h")?;
                Ok(terminal)
            }
        }

        pub fn clear_input(&mut self) {
            self.input.clear();
            self.high_surrogate = None;
        }

        pub fn set_prompt_prefix(&mut self, prefix: &str, highlight: bool) -> io::Result<()> {
            if self.prompt_prefix != prefix || self.prompt_highlight != highlight {
                self.prompt_prefix = prefix.to_owned();
                self.prompt_highlight = highlight;
                self.render_prompt()?;
            }
            Ok(())
        }

        pub fn set_title(&mut self, title: &str) -> io::Result<()> {
            let title = sanitize(title);
            if self.current_title == title {
                return Ok(());
            }
            let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
            // SAFETY: wide is a live NUL-terminated string.
            if unsafe { SetConsoleTitleW(wide.as_ptr()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            self.current_title = title;
            Ok(())
        }

        pub fn poll_events(&mut self) -> io::Result<Vec<InputEvent>> {
            let mut available = 0;
            // SAFETY: input_handle is a borrowed console input handle; outputs
            // point to initialized, correctly sized writable local storage.
            if unsafe { GetNumberOfConsoleInputEvents(self.input_handle, &mut available) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if available == 0 {
                return Ok(Vec::new());
            }
            // Bound each poll so pasted input cannot starve the dashboard loop.
            let mut records = vec![INPUT_RECORD::default(); available.min(256) as usize];
            let mut read = 0;
            if unsafe {
                ReadConsoleInputW(
                    self.input_handle,
                    records.as_mut_ptr(),
                    records.len() as u32,
                    &mut read,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let mut commands = Vec::new();
            let mut changed = false;
            for record in records.into_iter().take(read as usize) {
                if record.EventType as u32 != KEY_EVENT {
                    continue;
                }
                // SAFETY: EventType identifies KeyEvent as the active union arm.
                let key = unsafe { record.Event.KeyEvent };
                if key.bKeyDown == 0 {
                    continue;
                }
                // SAFETY: ReadConsoleInputW returns Unicode key data.
                let unit = unsafe { key.uChar.UnicodeChar };
                if unit == 3
                    || (key.wVirtualKeyCode == 0x43
                        && key.dwControlKeyState & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0)
                {
                    commands.push(InputEvent::Quit);
                    continue;
                }
                for _ in 0..key.wRepeatCount.max(1) {
                    match key.wVirtualKeyCode {
                        0x0d => {
                            self.high_surrogate = None;
                            let command = std::mem::take(&mut self.input);
                            if !self.alternate {
                                self.write(&format!("\r\x1b[2K> {}\r\n", sanitize(&command)))?;
                            }
                            commands.push(InputEvent::Enter(command));
                            changed = true;
                        }
                        0x08 => {
                            self.high_surrogate = None;
                            self.input.pop();
                            changed = true;
                        }
                        0x1b => {
                            self.clear_input();
                            commands.push(InputEvent::Escape);
                            changed = true;
                        }
                        0x26 => commands.push(InputEvent::Up),
                        0x28 => commands.push(InputEvent::Down),
                        0x25 => commands.push(InputEvent::Left),
                        0x27 => commands.push(InputEvent::Right),
                        _ if unit >= 0x20 && unit != 0x7f => {
                            if (0xd800..=0xdbff).contains(&unit) {
                                self.high_surrogate = Some(unit);
                                continue;
                            }
                            let character = if (0xdc00..=0xdfff).contains(&unit) {
                                self.high_surrogate.take().and_then(|high| {
                                    char::from_u32(
                                        0x10000 + ((high as u32 - 0xd800) << 10) + unit as u32
                                            - 0xdc00,
                                    )
                                })
                            } else {
                                self.high_surrogate = None;
                                char::from_u32(unit as u32)
                            };
                            if let Some(c) = character.filter(|c| !c.is_control()) {
                                if self.input.len() < 16_384 {
                                    self.input.push(c);
                                }
                                changed = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
            if changed {
                self.render_prompt()?;
            }
            Ok(commands)
        }

        pub fn draw(
            &mut self,
            lines: &[String],
            title: &str,
            highlight: Option<usize>,
        ) -> io::Result<()> {
            self.set_title(title)?;
            if !self.alternate {
                self.write("\x1b[?1049h\x1b[2J\x1b[H")?;
                self.alternate = true;
            }
            let (columns, rows) = self.dimensions()?;
            let mut frame = String::from("\x1b[?25l\x1b[H");
            for row in 1..rows {
                frame.push_str(&format!("\x1b[{row};1H\x1b[2K"));
                if row == 1 {
                    frame.push_str("\x1b[1;36m");
                    frame.push_str(&clip(title, columns));
                    frame.push_str("\x1b[0m");
                } else if let Some(line) = lines.get(row - 2) {
                    if highlight == Some(row - 2) {
                        frame.push_str(SELECTION_STYLE);
                    }
                    frame.push_str(&clip(line, columns));
                    if highlight == Some(row - 2) {
                        frame.push_str("\x1b[0m");
                    }
                }
            }
            frame.push_str(&format!(
                "\x1b[{rows};1H\x1b[2K{}\x1b[?25h",
                prompt(&self.prompt_prefix, &self.input, columns)
            ));
            self.write(&frame)
        }

        /// Switches to the primary buffer once. Later updates use append_log;
        /// do not call this again for every refresh, as it reprints history.
        pub fn switch_log(&mut self, history: &[String], title: &str) -> io::Result<()> {
            self.set_title(title)?;
            if self.alternate {
                self.write("\x1b[?1049l")?;
                self.alternate = false;
            }
            self.write("\x1b[0m\x1b[2J\x1b[H\x1b[?25h")?;
            self.write(&format!("{}\r\n", sanitize(title)))?;
            for line in history {
                self.write_log_text(line)?;
            }
            self.render_prompt()
        }

        pub fn append_log(&mut self, text: &str) -> io::Result<()> {
            if self.alternate {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Switch to the log page before appending",
                ));
            }
            self.write("\r\x1b[2K")?;
            self.write_log_text(text)?;
            self.render_prompt()
        }

        fn write_log_text(&mut self, text: &str) -> io::Result<()> {
            if text.is_empty() {
                return self.write("\r\n");
            }
            for line in text.lines() {
                self.write(&format!("{}\r\n", sanitize(line)))?;
            }
            Ok(())
        }

        fn render_prompt(&mut self) -> io::Result<()> {
            let (columns, rows) = self.dimensions()?;
            let position = if self.alternate {
                format!("\x1b[{rows};1H")
            } else {
                "\r".into()
            };
            self.write(&format!(
                "{position}\x1b[2K{}{}\x1b[0m\x1b[?25h",
                if self.prompt_highlight {
                    SELECTION_STYLE
                } else {
                    ""
                },
                prompt(&self.prompt_prefix, &self.input, columns)
            ))
        }

        fn dimensions(&self) -> io::Result<(usize, usize)> {
            let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
            // SAFETY: output is a valid console handle and info is writable.
            if unsafe { GetConsoleScreenBufferInfo(self.output_handle, &mut info) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // Leave the last column unused to avoid automatic line wrapping.
            let columns =
                (i32::from(info.srWindow.Right) - i32::from(info.srWindow.Left)).max(1) as usize;
            let rows = (i32::from(info.srWindow.Bottom) - i32::from(info.srWindow.Top) + 1).max(1)
                as usize;
            Ok((columns, rows))
        }

        fn write(&mut self, text: &str) -> io::Result<()> {
            let wide: Vec<u16> = text.encode_utf16().collect();
            let mut start = 0;
            while start < wide.len() {
                let mut end = (start + 8192).min(wide.len());
                if end < wide.len() && (0xd800..=0xdbff).contains(&wide[end - 1]) {
                    end -= 1;
                }
                let mut written = 0;
                // SAFETY: the UTF-16 slice remains live and its size matches the
                // count. WriteConsoleW consumes no memory past that slice.
                if unsafe {
                    WriteConsoleW(
                        self.output_handle,
                        wide[start..end].as_ptr(),
                        (end - start) as u32,
                        &mut written,
                        std::ptr::null(),
                    )
                } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                if written == 0 {
                    return Err(io::ErrorKind::WriteZero.into());
                }
                start += written as usize;
            }
            Ok(())
        }
    }

    impl Drop for Terminal {
        fn drop(&mut self) {
            let restore = if self.alternate {
                "\x1b[0m\x1b[?25h\x1b[?1049l"
            } else {
                "\r\x1b[2K\x1b[0m\x1b[?25h\r\n"
            };
            let _ = self.write(restore);
            // SAFETY: borrowed standard handles remain valid for this scope;
            // saved modes/title came from this console before modification.
            unsafe {
                SetConsoleMode(self.input_handle, self.original_input);
                SetConsoleMode(self.output_handle, self.original_output);
                SetConsoleTitleW(self.original_title.as_ptr());
            }
        }
    }
}

#[cfg(windows)]
pub use platform::Terminal;

#[cfg(not(windows))]
pub struct Terminal;
#[cfg(not(windows))]
impl Terminal {
    pub fn enter() -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "The native terminal requires Windows",
        ))
    }
    pub fn clear_input(&mut self) {}
    pub fn set_title(&mut self, _: &str) -> io::Result<()> {
        Self::enter().map(|_| ())
    }
    pub fn poll_events(&mut self) -> io::Result<Vec<InputEvent>> {
        Self::enter().map(|_| Vec::new())
    }
    pub fn draw(&mut self, _: &[String], _: &str, _: Option<usize>) -> io::Result<()> {
        Self::enter().map(|_| ())
    }
    pub fn switch_log(&mut self, _: &[String], _: &str) -> io::Result<()> {
        Self::enter().map(|_| ())
    }
    pub fn append_log(&mut self, _: &str) -> io::Result<()> {
        Self::enter().map(|_| ())
    }
    pub fn set_prompt_prefix(&mut self, _: &str, _: bool) -> io::Result<()> {
        Self::enter().map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrusted_text_cannot_inject_terminal_controls() {
        assert_eq!(sanitize("game\x1b[2J\r\n\u{009b}title"), "game [2J   title");
    }

    #[test]
    fn cjk_text_and_input_tail_fit_the_viewport() {
        assert_eq!(clip("雷神abc", 5), "雷神a");
        assert_eq!(clip("a\u{0301}bc", 2), "a\u{0301}b");
        assert_eq!(prompt("", "long 雷神", 6), "> 雷神");
        assert_eq!(prompt("", "abcdef", 5), "> def");
        assert_eq!(prompt("分钟 ", "123", 9), "分钟 > 23");
        for width in 0..12 {
            let rendered = prompt("", "中文abc😀", width);
            assert!(rendered.chars().map(char_width).sum::<usize>() <= width);
        }
    }
}
