//! Mouse and keyboard input to windows, and what the gadgets do with it.
//!
//! Ported from `GameWindowManager::winProcessMouseEvent` (the window under the
//! cursor, mouse capture while a button is held, entering and leaving), the
//! gadgets' input functions in `GameClient/GUI/Gadget/` (`GadgetPushButtonInput`
//! with `CHECK_LIKE` and `ON_MOUSE_DOWN`, `GadgetCheckBoxInput`,
//! `GadgetRadioButtonInput` with its groups, the slider inputs, `GadgetListBoxInput`,
//! `GadgetComboBoxInput`, `GadgetTextEntryInput`) and `winSendSystemMsg` to a
//! gadget's owner (its parent).
//!
//! Gadgets report to their owner with [`Message`]s, which the shell hands to the
//! owner's system callback (`PassSelectedButtonsToParentSystem` and
//! `PassMessagesToParentSystem` hand them on up). Interface sounds the original
//! plays from input (`GUIClick`) come out as [`UiEvent::Sound`].
use crate::{WindowId, WindowManager, state, status};

/// A gadget's own state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gadget {
    /// Sliders: value, min, max.
    pub value: i32,
    pub min: i32,
    pub max: i32,
    /// List and combo boxes: rows of columns, and the selected row.
    pub rows: Vec<Vec<String>>,
    pub selected: Option<usize>,
    /// Combo boxes: the drop-down list is open.
    pub open: bool,
    /// Text entry: the text and its limit.
    pub entry: String,
    pub max_len: usize,
    /// Radio buttons: the group.
    pub group: i32,
}

/// `GBM_*`, `GSM_*`, `GLM_*`, `GCM_*`, `GEM_*` messages to a gadget's owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// `GBM_SELECTED`: a button, check box or radio button was clicked.
    Selected(WindowId),
    /// `GBM_MOUSE_ENTERING` / `GBM_MOUSE_LEAVING`.
    MouseEntering(WindowId),
    MouseLeaving(WindowId),
    /// `GSM_SLIDER_TRACK`: a slider's value changed.
    SliderTrack(WindowId, i32),
    /// `GLM_SELECTED` / `GCM_SELECTED`: a list or combo box row was chosen.
    ListSelected(WindowId, Option<usize>),
    /// `GEM_UPDATE_TEXT` / `GEM_EDIT_DONE`.
    EditUpdate(WindowId),
    EditDone(WindowId),
}

/// What input produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEvent {
    /// A message for `owner`'s system callback.
    System { owner: WindowId, message: Message },
    /// An interface sound event to play (`AudioEventRTS` by name).
    Sound(String),
}

/// Input to the window system, in screen pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    MouseMove([i32; 2]),
    LeftDown([i32; 2]),
    LeftUp([i32; 2]),
    /// A typed character.
    Char(char),
    Backspace,
    Enter,
}

/// The manager's input state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputState {
    /// `m_mouseCaptor`: the window a held button belongs to.
    pub captor: Option<WindowId>,
    /// The window under the mouse last time (`m_grabWindow` for entering/leaving).
    pub under: Option<WindowId>,
    /// `m_keyboardFocus`.
    pub focus: Option<WindowId>,
}

/// A list row's height for hit testing (the original measures the font).
pub const ROW_HEIGHT: i32 = 16;

impl WindowManager {
    /// Fills each gadget's data from its script (`LISTBOXDATA`, `SLIDERDATA`,
    /// `TEXTENTRYDATA`, `RADIOBUTTONDATA`).
    pub fn init_gadgets(&mut self) {
        for w in &mut self.windows {
            let g = &mut w.gadget;
            if let Some(f) = w.script.field("SLIDERDATA") {
                g.min = f.int("MINVALUE").unwrap_or(0);
                g.max = f.int("MAXVALUE").unwrap_or(100);
                g.value = g.min;
            }
            if let Some(f) = w.script.field("TEXTENTRYDATA") {
                g.max_len = f.int("MAXLEN").unwrap_or(64).max(0) as usize;
            }
            if let Some(f) = w.script.field("COMBOBOXDATA") {
                g.max_len = f.int("MAXCHARS").unwrap_or(64).max(0) as usize;
            }
            if let Some(f) = w.script.field("RADIOBUTTONDATA") {
                g.group = f.int("GROUP").unwrap_or(0);
            }
        }
    }

    fn owner(&self, id: WindowId) -> WindowId {
        self.windows[id].parent.unwrap_or(id)
    }
    fn system(&self, id: WindowId, message: Message) -> UiEvent {
        UiEvent::System { owner: self.owner(id), message }
    }
    fn is(&self, id: WindowId, kind: &str) -> bool {
        self.windows[id].kind == kind
    }

    /// The gadget window for a point: the window itself, or the gadget it belongs
    /// to when it has no input of its own (`NOINPUT` labels on buttons).
    fn target(&self, p: [i32; 2]) -> Option<WindowId> {
        let mut id = self.window_at(p)?;
        while self.windows[id].status & status::NO_INPUT != 0 {
            id = self.windows[id].parent?;
        }
        Some(id)
    }

    /// `winProcessMouseEvent` and the gadgets' input functions.
    pub fn input(&mut self, input: &Input) -> Vec<UiEvent> {
        let mut out = vec![];
        match *input {
            Input::MouseMove(p) => {
                let under = self.input_state.captor.or_else(|| self.target(p));
                if under != self.input_state.under {
                    if let Some(old) = self.input_state.under.filter(|&o| o < self.windows.len()) {
                        self.leave(old, &mut out);
                    }
                    if let Some(new) = under {
                        self.enter(new, &mut out);
                    }
                    self.input_state.under = under;
                }
                if let Some(c) = self.input_state.captor
                    && (self.is(c, "HORZSLIDER") || self.is(c, "VERTSLIDER"))
                {
                    self.slide(c, p, &mut out);
                }
            }
            Input::LeftDown(p) => {
                let Some(id) = self.target(p) else { return out };
                if self.windows[id].status & status::ENABLED == 0 {
                    return out;
                }
                self.input_state.captor = Some(id);
                self.input_state.focus = (self.is(id, "ENTRYFIELD")).then_some(id);
                self.left_down(id, p, &mut out);
            }
            Input::LeftUp(p) => {
                let captor = self.input_state.captor.take();
                if let Some(id) = captor {
                    let inside = self.target(p) == Some(id);
                    self.left_up(id, inside, &mut out);
                }
            }
            Input::Char(c) => {
                if let Some(f) = self.input_state.focus {
                    let g = &mut self.windows[f].gadget;
                    if g.entry.chars().count() < g.max_len.max(1) && !c.is_control() {
                        g.entry.push(c);
                        out.push(self.system(f, Message::EditUpdate(f)));
                    }
                }
            }
            Input::Backspace => {
                if let Some(f) = self.input_state.focus
                    && self.windows[f].gadget.entry.pop().is_some()
                {
                    out.push(self.system(f, Message::EditUpdate(f)));
                }
            }
            Input::Enter => {
                if let Some(f) = self.input_state.focus {
                    out.push(self.system(f, Message::EditDone(f)));
                }
            }
        }
        out
    }

    fn enter(&mut self, id: WindowId, out: &mut Vec<UiEvent>) {
        let w = &mut self.windows[id];
        if w.status & status::ENABLED != 0 && w.kind != "USER" {
            w.state |= state::HILITED;
            out.push(self.system(id, Message::MouseEntering(id)));
        }
    }
    fn leave(&mut self, id: WindowId, out: &mut Vec<UiEvent>) {
        let check_like = self.windows[id].status & crate::status::CHECK_LIKE != 0;
        let w = &mut self.windows[id];
        if w.state & state::HILITED != 0 {
            w.state &= !state::HILITED;
            // A pressed (not check-like) button lets go when the mouse leaves.
            if w.kind == "PUSHBUTTON" && !check_like {
                w.state &= !state::SELECTED;
            }
            out.push(self.system(id, Message::MouseLeaving(id)));
        }
    }

    fn left_down(&mut self, id: WindowId, p: [i32; 2], out: &mut Vec<UiEvent>) {
        let kind = self.windows[id].kind.clone();
        let st = self.windows[id].status;
        match kind.as_str() {
            "PUSHBUTTON" => {
                out.push(UiEvent::Sound("GUIClick".into()));
                let check_like = st & crate::status::CHECK_LIKE != 0;
                let w = &mut self.windows[id];
                if check_like {
                    w.state ^= state::SELECTED;
                } else {
                    w.state |= state::SELECTED;
                }
                // buttonTriggersOnMouseDown
                if check_like || st & crate::status::ON_MOUSE_DOWN != 0 {
                    out.push(self.system(id, Message::Selected(id)));
                }
            }
            "CHECKBOX" => {
                self.windows[id].state ^= state::SELECTED;
                out.push(self.system(id, Message::Selected(id)));
            }
            "RADIOBUTTON" => {
                // Only one of a group under the same parent is selected.
                let (parent, group) = (self.windows[id].parent, self.windows[id].gadget.group);
                if let Some(parent) = parent {
                    for c in self.windows[parent].children.clone() {
                        if self.is(c, "RADIOBUTTON") && self.windows[c].gadget.group == group {
                            self.windows[c].state &= !state::SELECTED;
                        }
                    }
                }
                self.windows[id].state |= state::SELECTED;
                out.push(self.system(id, Message::Selected(id)));
            }
            "HORZSLIDER" | "VERTSLIDER" => self.slide(id, p, out),
            "SCROLLLISTBOX" => {
                let top = self.screen_position(id)[1];
                let row = ((p[1] - top) / ROW_HEIGHT).max(0) as usize;
                let g = &mut self.windows[id].gadget;
                g.selected = (row < g.rows.len()).then_some(row);
                let sel = g.selected;
                if sel.is_some() {
                    out.push(UiEvent::Sound("GUIComboBoxClick".into()));
                }
                out.push(self.system(id, Message::ListSelected(id, sel)));
            }
            "COMBOBOX" => {
                let o = self.screen_position(id);
                let h = self.windows[id].size[1];
                let g = &mut self.windows[id].gadget;
                if g.open && p[1] > o[1] + h {
                    // A row of the open drop-down list.
                    let row = ((p[1] - o[1] - h) / ROW_HEIGHT) as usize;
                    if row < g.rows.len() {
                        g.selected = Some(row);
                    }
                    g.open = false;
                    let sel = g.selected;
                    if sel.is_some() {
                        out.push(UiEvent::Sound("GUIComboBoxClick".into()));
                    }
                    out.push(self.system(id, Message::ListSelected(id, sel)));
                } else {
                    g.open = !g.open;
                    out.push(UiEvent::Sound("GUIClick".into()));
                }
            }
            _ => {}
        }
    }

    fn left_up(&mut self, id: WindowId, inside: bool, out: &mut Vec<UiEvent>) {
        if self.is(id, "PUSHBUTTON") {
            let st = self.windows[id].status;
            let check_like = st & crate::status::CHECK_LIKE != 0;
            if self.windows[id].state & state::SELECTED != 0 && !check_like {
                if inside && st & crate::status::ON_MOUSE_DOWN == 0 {
                    out.push(self.system(id, Message::Selected(id)));
                }
                self.windows[id].state &= !state::SELECTED;
            }
        }
    }

    /// Sliders: the value from the mouse along the track.
    fn slide(&mut self, id: WindowId, p: [i32; 2], out: &mut Vec<UiEvent>) {
        let o = self.screen_position(id);
        let s = self.windows[id].size;
        let vertical = self.is(id, "VERTSLIDER");
        let (pos, len) = if vertical { (s[1] - (p[1] - o[1]), s[1]) } else { (p[0] - o[0], s[0]) };
        let g = &mut self.windows[id].gadget;
        let span = (g.max - g.min).max(0);
        let v = g.min + (pos.clamp(0, len.max(1)) * span + len.max(1) / 2) / len.max(1);
        if v != g.value {
            g.value = v;
            out.push(self.system(id, Message::SliderTrack(id, v)));
        }
    }

    /// A message for `owner`'s system callback, handed up through windows whose
    /// system callback only passes messages on, to the window that handles it.
    pub fn handler(&self, owner: WindowId) -> WindowId {
        let mut id = owner;
        loop {
            let passes = self.windows[id].script.callback("SYSTEMCALLBACK").is_some_and(|c| {
                c.eq_ignore_ascii_case("PassSelectedButtonsToParentSystem")
                    || c.eq_ignore_ascii_case("PassMessagesToParentSystem")
            });
            match (passes, self.windows[id].parent) {
                (true, Some(p)) => id = p,
                _ => return id,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ih_formats::wnd;

    const SRC: &[u8] = br#"WINDOW
WINDOWTYPE = USER;
SCREENRECT = UPPERLEFT: 0 0, BOTTOMRIGHT: 800 600, CREATIONRESOLUTION: 800 600;
NAME = "T.wnd:Root";
STATUS = ENABLED;
SYSTEMCALLBACK = "TestMenuSystem";
CHILD
WINDOW
WINDOWTYPE = USER;
SCREENRECT = UPPERLEFT: 0 0, BOTTOMRIGHT: 400 300, CREATIONRESOLUTION: 800 600;
NAME = "T.wnd:Panel";
STATUS = ENABLED;
SYSTEMCALLBACK = "PassSelectedButtonsToParentSystem";
CHILD
WINDOW
WINDOWTYPE = PUSHBUTTON;
SCREENRECT = UPPERLEFT: 10 10, BOTTOMRIGHT: 110 40, CREATIONRESOLUTION: 800 600;
NAME = "T.wnd:Button";
STATUS = ENABLED;
STYLE = PUSHBUTTON+MOUSETRACK;
END
ENDALLCHILDREN
END
WINDOW
WINDOWTYPE = RADIOBUTTON;
SCREENRECT = UPPERLEFT: 500 10, BOTTOMRIGHT: 520 30, CREATIONRESOLUTION: 800 600;
NAME = "T.wnd:RadioA";
STATUS = ENABLED;
RADIOBUTTONDATA = GROUP: 1;
END
WINDOW
WINDOWTYPE = RADIOBUTTON;
SCREENRECT = UPPERLEFT: 500 40, BOTTOMRIGHT: 520 60, CREATIONRESOLUTION: 800 600;
NAME = "T.wnd:RadioB";
STATUS = ENABLED;
RADIOBUTTONDATA = GROUP: 1;
END
WINDOW
WINDOWTYPE = HORZSLIDER;
SCREENRECT = UPPERLEFT: 500 100, BOTTOMRIGHT: 600 110, CREATIONRESOLUTION: 800 600;
NAME = "T.wnd:Slider";
STATUS = ENABLED;
SLIDERDATA = MINVALUE: 0, MAXVALUE: 10;
END
WINDOW
WINDOWTYPE = ENTRYFIELD;
SCREENRECT = UPPERLEFT: 500 200, BOTTOMRIGHT: 600 220, CREATIONRESOLUTION: 800 600;
NAME = "T.wnd:Entry";
STATUS = ENABLED;
TEXTENTRYDATA = MAXLEN: 3;
END
ENDALLCHILDREN
END
"#;

    fn ui() -> WindowManager {
        let mut m = WindowManager::new([800, 600]);
        m.load(&wnd::parse(SRC).unwrap());
        m.init_gadgets();
        m
    }
    fn selected(ev: &[UiEvent]) -> Vec<WindowId> {
        ev.iter()
            .filter_map(|e| match e {
                UiEvent::System { message: Message::Selected(w), .. } => Some(*w),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_click_selects_on_release_and_reaches_the_menu() {
        let mut m = ui();
        let b = m.find("T.wnd:Button").unwrap();
        let ev = m.input(&Input::MouseMove([20, 20]));
        assert!(m.windows[b].state & state::HILITED != 0, "{ev:?}");
        let down = m.input(&Input::LeftDown([20, 20]));
        assert!(down.contains(&UiEvent::Sound("GUIClick".into())));
        assert!(selected(&down).is_empty(), "not until release");
        let up = m.input(&Input::LeftUp([20, 20]));
        assert_eq!(selected(&up), [b]);
        let UiEvent::System { owner, .. } = &up[0] else { panic!() };
        // The panel passes it on to the root, whose callback handles it.
        assert_eq!(m.handler(*owner), m.find("T.wnd:Root").unwrap());
        // Released outside: no click.
        m.input(&Input::LeftDown([20, 20]));
        assert!(selected(&m.input(&Input::LeftUp([300, 300]))).is_empty());
    }

    #[test]
    fn radio_groups_sliders_and_entry_fields() {
        let mut m = ui();
        let (a, b) = (m.find("T.wnd:RadioA").unwrap(), m.find("T.wnd:RadioB").unwrap());
        m.input(&Input::LeftDown([510, 20]));
        m.input(&Input::LeftUp([510, 20]));
        m.input(&Input::LeftDown([510, 50]));
        assert_eq!(m.windows[a].state & state::SELECTED, 0);
        assert_ne!(m.windows[b].state & state::SELECTED, 0);
        m.input(&Input::LeftUp([510, 50]));
        let s = m.find("T.wnd:Slider").unwrap();
        m.input(&Input::LeftDown([550, 105]));
        assert_eq!(m.windows[s].gadget.value, 5);
        m.input(&Input::MouseMove([700, 105]));
        assert_eq!(m.windows[s].gadget.value, 10, "dragging clamps to the end");
        m.input(&Input::LeftUp([700, 105]));
        let e = m.find("T.wnd:Entry").unwrap();
        m.input(&Input::LeftDown([550, 210]));
        m.input(&Input::LeftUp([550, 210]));
        for c in "abcd".chars() {
            m.input(&Input::Char(c));
        }
        m.input(&Input::Backspace);
        assert_eq!(m.windows[e].gadget.entry, "ab", "MAXLEN 3, then one deleted");
        assert!(matches!(m.input(&Input::Enter)[0], UiEvent::System { message: Message::EditDone(_), .. }));
    }
}
