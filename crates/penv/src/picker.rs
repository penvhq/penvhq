//! The choice prompts: one item, or several ticked with space. The state and
//! the frame are pure; `tty` reads the keys and draws the frames.

use console::style;

use crate::keys::Key;

/// Rows shown at once; a longer list scrolls with the cursor.
const WINDOW: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Continue,
    Submit,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct Picker {
    pub title: String,
    pub items: Vec<String>,
    pub cursor: usize,
    /// `Some` for a multiselect: which items are ticked.
    pub ticked: Option<Vec<bool>>,
    top: usize,
}

impl Picker {
    pub fn one(title: &str, items: &[String], initial: Option<usize>) -> Picker {
        let cursor = initial.filter(|i| *i < items.len()).unwrap_or(0);
        Picker {
            title: title.to_string(),
            items: items.to_vec(),
            cursor,
            ticked: None,
            top: cursor.saturating_sub(WINDOW - 1),
        }
    }

    pub fn many(title: &str, items: &[String], chosen: &[usize]) -> Picker {
        let ticked = (0..items.len()).map(|i| chosen.contains(&i)).collect();
        Picker {
            ticked: Some(ticked),
            ..Picker::one(title, items, None)
        }
    }

    /// Up and Down stop at the ends, as the prompts always have.
    pub fn on(&mut self, key: Key) -> Step {
        match key {
            Key::Up => self.cursor = self.cursor.saturating_sub(1),
            Key::Down if self.cursor + 1 < self.items.len() => self.cursor += 1,
            Key::Space => {
                if let Some(ticked) = self.ticked.as_mut().and_then(|t| t.get_mut(self.cursor)) {
                    *ticked = !*ticked;
                }
            }
            Key::Enter if !self.items.is_empty() => return Step::Submit,
            Key::Escape | Key::Interrupt => return Step::Cancel,
            _ => {}
        }
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + WINDOW {
            self.top = self.cursor + 1 - WINDOW;
        }
        Step::Continue
    }

    /// The ticked items, in list order.
    pub fn chosen(&self) -> Vec<usize> {
        self.ticked
            .as_ref()
            .map(|t| (0..t.len()).filter(|i| t[*i]).collect())
            .unwrap_or_default()
    }

    /// The prompt as it stands, one entry per screen line, each cut to `width`
    /// columns so no line wraps and the redraw always knows its height.
    pub fn frame(&self, width: usize) -> Vec<String> {
        let fit = |text: &str, room: usize| console::truncate_str(text, room, "…").into_owned();
        let room = width.saturating_sub(6).max(8);
        let mut lines = vec![format!("{}  {}", style("◆").cyan(), fit(&self.title, room))];
        let end = (self.top + WINDOW).min(self.items.len());
        if self.top > 0 {
            lines.push(format!("{}  {}", style("│").cyan(), style("…").dim()));
        }
        for index in self.top..end {
            let here = index == self.cursor;
            let mark = match &self.ticked {
                Some(ticked) if ticked[index] => style("◼").green().to_string(),
                Some(_) => style("◻").dim().to_string(),
                None if here => style("●").green().to_string(),
                None => style("○").dim().to_string(),
            };
            let item = fit(&self.items[index], room);
            let item = if here {
                item
            } else {
                style(item).dim().to_string()
            };
            lines.push(format!("{}  {mark} {item}", style("│").cyan()));
        }
        if end < self.items.len() {
            lines.push(format!("{}  {}", style("│").cyan(), style("…").dim()));
        }
        let hint = match self.ticked {
            Some(_) => "↑/↓ to move, space to tick, enter to confirm",
            None => "↑/↓ to move, enter to choose",
        };
        lines.push(format!("{}  {}", style("└").cyan(), style(hint).dim()));
        lines
    }

    /// What stays on screen once the prompt is answered or left.
    pub fn summary(&self, step: &Step) -> Vec<String> {
        let answer = match (step, &self.ticked) {
            (Step::Cancel, _) => style("cancelled").dim().to_string(),
            (_, Some(_)) => {
                let names: Vec<&str> = self
                    .chosen()
                    .iter()
                    .map(|i| self.items[*i].as_str())
                    .collect();
                if names.is_empty() {
                    style("none").dim().to_string()
                } else {
                    style(names.join(", ")).dim().to_string()
                }
            }
            (_, None) => style(&self.items[self.cursor]).dim().to_string(),
        };
        vec![
            format!("{}  {}", style("◇").green(), self.title),
            format!("{}  {answer}", style("│").dim()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("item {i}")).collect()
    }

    #[test]
    fn down_down_up_enter_chooses_the_second_item() {
        let mut picker = Picker::one("Which?", &items(4), None);
        for key in [Key::Down, Key::Down, Key::Up] {
            assert_eq!(picker.on(key), Step::Continue);
        }
        assert_eq!(picker.on(Key::Enter), Step::Submit);
        assert_eq!(picker.cursor, 1);
    }

    #[test]
    fn the_ends_hold() {
        let mut picker = Picker::one("Which?", &items(2), Some(1));
        picker.on(Key::Down);
        assert_eq!(picker.cursor, 1);
        picker.on(Key::Up);
        picker.on(Key::Up);
        assert_eq!(picker.cursor, 0);
        assert_eq!(Picker::one("Which?", &items(2), Some(9)).cursor, 0);
    }

    #[test]
    fn esc_and_ctrl_c_cancel_and_other_keys_do_nothing() {
        let mut picker = Picker::one("Which?", &items(3), None);
        for key in [Key::Other, Key::Char('x'), Key::Space] {
            assert_eq!(picker.on(key), Step::Continue);
            assert_eq!(picker.cursor, 0);
        }
        assert_eq!(picker.on(Key::Escape), Step::Cancel);
        assert_eq!(picker.on(Key::Interrupt), Step::Cancel);
    }

    #[test]
    fn space_ticks_in_a_multiselect() {
        let mut picker = Picker::many("Which?", &items(3), &[2]);
        picker.on(Key::Space);
        picker.on(Key::Down);
        picker.on(Key::Down);
        picker.on(Key::Space);
        assert_eq!(picker.on(Key::Enter), Step::Submit);
        assert_eq!(picker.chosen(), [0]);
    }

    #[test]
    fn a_long_list_scrolls_with_the_cursor_and_every_line_fits() {
        console::set_colors_enabled(false);
        let mut picker = Picker::one("Which?", &items(30), None);
        for _ in 0..15 {
            picker.on(Key::Down);
        }
        let frame = picker.frame(40);
        assert_eq!(frame.len(), 1 + 1 + WINDOW + 1 + 1, "{frame:#?}");
        assert!(frame.iter().any(|l| l.contains("● item 15")), "{frame:#?}");
        assert!(!frame.iter().any(|l| l.contains("item 0")), "{frame:#?}");

        let wide = Picker::one(&"t".repeat(200), &["x".repeat(200)], None);
        for line in wide.frame(40) {
            assert!(console::measure_text_width(&line) <= 40, "{line}");
        }
    }
}
