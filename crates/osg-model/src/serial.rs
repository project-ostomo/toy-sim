use serde::{Deserialize, Serialize};

pub const COLUMNS: usize = 64;
pub const ROWS: usize = 8;
pub const DEFAULT_COLOR: [u8; 3] = [224, 224, 218];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub character: char,
    pub foreground: [u8; 3],
    pub background: Option<[u8; 3]>,
    pub bold: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            character: ' ',
            foreground: DEFAULT_COLOR,
            background: None,
            bold: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub cells: Vec<Cell>,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            cells: vec![Cell::default(); COLUMNS * ROWS],
        }
    }
}

#[derive(Clone, Default)]
pub struct Terminal {
    pub screen: Screen,
    cursor: (usize, usize),
    saved: (usize, usize),
    style: Cell,
    escape: Vec<u8>,
    utf8: Vec<u8>,
    osc: bool,
    osc_escape: bool,
}

impl Terminal {
    pub fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.osc {
                if byte == 7 || (self.osc_escape && byte == b'\\') {
                    self.osc = false;
                }
                self.osc_escape = byte == 27;
                continue;
            }
            if !self.escape.is_empty() {
                self.escape.push(byte);
                if self.escape.len() == 2 {
                    match byte {
                        b'[' => continue,
                        b']' => {
                            self.osc = true;
                            self.osc_escape = false;
                        }
                        b'7' => self.saved = self.cursor,
                        b'8' => self.cursor = self.saved,
                        b'c' => *self = Self::default(),
                        _ => {}
                    }
                    self.escape.clear();
                } else if (0x40..=0x7e).contains(&byte) {
                    let parameters = self.escape[2..self.escape.len() - 1].to_vec();
                    self.csi(&parameters, byte);
                    self.escape.clear();
                } else if self.escape.len() >= 64 {
                    self.escape.clear();
                }
                continue;
            }
            match byte {
                27 => {
                    self.utf8.clear();
                    self.escape.push(byte);
                }
                b'\r' => self.cursor.0 = 0,
                b'\n' => {
                    self.cursor.0 = 0;
                    self.newline();
                }
                b'\x08' => self.cursor.0 = self.cursor.0.saturating_sub(1),
                b'\t' => self.cursor.0 = ((self.cursor.0 / 8 + 1) * 8).min(COLUMNS - 1),
                0..=31 | 127 => {}
                _ => {
                    self.utf8.push(byte);
                    match std::str::from_utf8(&self.utf8) {
                        Ok(text) => {
                            let character = text.chars().next().unwrap();
                            self.utf8.clear();
                            self.put(character);
                        }
                        Err(error) if error.error_len().is_some() || self.utf8.len() >= 4 => {
                            self.utf8.clear();
                            self.put('\u{fffd}');
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn blank(&self) -> Cell {
        Cell {
            character: ' ',
            ..self.style
        }
    }

    fn newline(&mut self) {
        self.cursor.1 += 1;
        if self.cursor.1 == ROWS {
            self.screen.cells.rotate_left(COLUMNS);
            let blank = self.blank();
            self.screen.cells[(ROWS - 1) * COLUMNS..].fill(blank);
            self.cursor.1 = ROWS - 1;
        }
    }

    fn put(&mut self, character: char) {
        if self.cursor.0 == COLUMNS {
            self.cursor.0 = 0;
            self.newline();
        }
        self.screen.cells[self.cursor.1 * COLUMNS + self.cursor.0] = Cell {
            character,
            ..self.style
        };
        self.cursor.0 += 1;
    }

    fn csi(&mut self, bytes: &[u8], command: u8) {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return;
        };
        if text.starts_with('?') {
            return;
        }
        let values: Vec<usize> = text
            .split(';')
            .map(|s| s.parse::<usize>().unwrap_or(0).min(65535))
            .collect();
        let first = values.first().copied().unwrap_or(0);
        let amount = first.max(1);
        self.cursor.0 = self.cursor.0.min(COLUMNS - 1);
        match command {
            b'A' => self.cursor.1 = self.cursor.1.saturating_sub(amount),
            b'B' => self.cursor.1 = (self.cursor.1 + amount).min(ROWS - 1),
            b'C' => self.cursor.0 = (self.cursor.0 + amount).min(COLUMNS - 1),
            b'D' => self.cursor.0 = self.cursor.0.saturating_sub(amount),
            b'G' => self.cursor.0 = (amount - 1).min(COLUMNS - 1),
            b'H' | b'f' => {
                self.cursor = (
                    (values.get(1).copied().unwrap_or(1).max(1) - 1).min(COLUMNS - 1),
                    (amount - 1).min(ROWS - 1),
                )
            }
            b's' => self.saved = self.cursor,
            b'u' => self.cursor = self.saved,
            b'J' | b'K' => {
                let index = self.cursor.1 * COLUMNS + self.cursor.0;
                let (start, end) = if command == b'K' {
                    (self.cursor.1 * COLUMNS, (self.cursor.1 + 1) * COLUMNS)
                } else {
                    (0, COLUMNS * ROWS)
                };
                let range = match first {
                    0 => index..end,
                    1 => start..index + 1,
                    2 | 3 => start..end,
                    _ => return,
                };
                let blank = self.blank();
                self.screen.cells[range].fill(blank);
            }
            b'm' => self.colors(&values),
            _ => {}
        }
    }

    fn colors(&mut self, values: &[usize]) {
        let mut index = 0;
        while index < values.len() {
            let code = values[index];
            match code {
                0 => self.style = Cell::default(),
                1 => self.style.bold = true,
                22 => self.style.bold = false,
                30..=37 => self.style.foreground = color(code - 30),
                90..=97 => self.style.foreground = color(code - 90 + 8),
                40..=47 => self.style.background = Some(color(code - 40)),
                100..=107 => self.style.background = Some(color(code - 100 + 8)),
                39 => self.style.foreground = DEFAULT_COLOR,
                49 => self.style.background = None,
                38 | 48 => {
                    let value = match values.get(index + 1) {
                        Some(5) if values.len() > index + 2 => {
                            index += 2;
                            Some(color(values[index].min(255)))
                        }
                        Some(2) if values.len() > index + 4 => {
                            let rgb = [
                                values[index + 2].min(255) as u8,
                                values[index + 3].min(255) as u8,
                                values[index + 4].min(255) as u8,
                            ];
                            index += 4;
                            Some(rgb)
                        }
                        _ => None,
                    };
                    if let Some(rgb) = value {
                        if code == 38 {
                            self.style.foreground = rgb;
                        } else {
                            self.style.background = Some(rgb);
                        }
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }
}

fn color(index: usize) -> [u8; 3] {
    const PALETTE: [[u8; 3]; 16] = [
        [0, 0, 0],
        [190, 45, 45],
        [70, 170, 80],
        [190, 160, 55],
        [70, 100, 180],
        [170, 75, 170],
        [60, 160, 165],
        [205, 205, 205],
        [105, 105, 105],
        [255, 90, 90],
        [130, 230, 130],
        [255, 220, 110],
        [130, 165, 255],
        [235, 140, 235],
        [130, 235, 240],
        [255, 255, 255],
    ];
    match index {
        0..=15 => PALETTE[index],
        16..=231 => {
            let n = index - 16;
            let channel = |v: usize| if v == 0 { 0 } else { (55 + 40 * v) as u8 };
            [channel(n / 36), channel(n / 6 % 6), channel(n % 6)]
        }
        _ => [8 + 10 * (index.min(255) - 232) as u8; 3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_controls_overwrite_reposition_and_color_without_scrollback() {
        let mut terminal = Terminal::default();
        terminal.write(b"Progress 0%\r\x1b[3");
        terminal.write(b"1mDone\x1b[K\x1b[0m\nNext");
        assert_eq!(terminal.screen.cells[0].character, 'D');
        assert_eq!(terminal.screen.cells[0].foreground, color(1));
        assert_eq!(terminal.screen.cells[4].character, ' ');
        terminal.write(b"\x1b[1;2H!\x1b[38;2;1;2;3mX");
        assert_eq!(terminal.screen.cells[1].character, '!');
        assert_eq!(terminal.screen.cells[2].foreground, [1, 2, 3]);
        terminal.write(&vec![b'\n'; 100]);
        assert_eq!(terminal.screen.cells.len(), COLUMNS * ROWS);
        assert!(
            terminal
                .screen
                .cells
                .iter()
                .all(|cell| cell.character == ' ')
        );
    }
}
