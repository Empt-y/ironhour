//! The window system: layouts loaded as live window trees.
//!
//! Ported from `GameClient/GUI/GameWindowManager.cpp` (`winCreate`,
//! `linkWindow`, `addWindowToParent`, `winGetWindowFromId`), `GameWindow.cpp`
//! (`winPointInChild`, `winGetScreenPosition`, `winHide`, `winEnable`) and the
//! layout half of `GameWindowManagerScript.cpp` (`parseScreenRect`'s scaling to
//! the display and parenting).
//!
//! Windows live in an arena. A parent's children are kept topmost first, as the
//! original prepends each new child (`addWindowToParent`), so the last window
//! in a file sits on top. Top-level windows are prepended the same way
//! (`linkWindow`). Modal windows, focus, input routing, gadgets, drawing and the
//! callback table come in later slices.
use ih_formats::wnd;

pub mod draw;

pub mod status {
    //! `WIN_STATUS_*` bits, in `wnd::STATUS_NAMES` order.
    pub const ACTIVE: u32 = 1 << 0;
    pub const ENABLED: u32 = 1 << 3;
    pub const HIDDEN: u32 = 1 << 4;
    pub const IMAGE: u32 = 1 << 7;
    pub const NO_INPUT: u32 = 1 << 9;
    pub const NO_FOCUS: u32 = 1 << 10;
}

/// A window's index in the manager.
pub type WindowId = usize;

/// A live window.
#[derive(Clone, Debug)]
pub struct Window {
    pub id: WindowId,
    pub parent: Option<WindowId>,
    /// Topmost first.
    pub children: Vec<WindowId>,
    /// `WINDOWTYPE`.
    pub kind: String,
    pub name: Option<String>,
    pub status: u32,
    pub style: u32,
    /// `m_region.lo`: relative to the parent (to the screen for top-level windows).
    pub position: [i32; 2],
    pub size: [i32; 2],
    /// The script entry it came from (fields, draw data, gadget data).
    pub script: wnd::Window,
}

/// `GameWindowManager`'s window list.
#[derive(Clone, Debug, Default)]
pub struct WindowManager {
    pub windows: Vec<Window>,
    /// Top-level windows, topmost first (`m_windowList`).
    pub top: Vec<WindowId>,
    /// The display size layouts are scaled to.
    pub display: [i32; 2],
}

impl WindowManager {
    pub fn new(display: [i32; 2]) -> Self {
        Self { display, ..Default::default() }
    }

    /// `winCreateFromScript`: every window of the layout, parented and scaled.
    /// Returns the layout's top-level windows in file order.
    pub fn load(&mut self, layout: &wnd::Layout) -> Vec<WindowId> {
        layout.windows.iter().map(|w| self.create(w, None)).collect()
    }

    fn screen_rect(&self, r: &wnd::ScreenRect) -> ([i32; 2], [i32; 2]) {
        // parseScreenRect: scale by display / creation resolution, truncating.
        let res = [r.creation_resolution[0].max(1), r.creation_resolution[1].max(1)];
        let scale = [self.display[0] as f32 / res[0] as f32, self.display[1] as f32 / res[1] as f32];
        let s = |v: i32, i: usize| (v as f32 * scale[i]) as i32;
        ([s(r.upper_left[0], 0), s(r.upper_left[1], 1)], [s(r.bottom_right[0], 0), s(r.bottom_right[1], 1)])
    }

    fn create(&mut self, w: &wnd::Window, parent: Option<WindowId>) -> WindowId {
        let (lo, hi) = self.screen_rect(&w.rect);
        let origin = parent.map_or([0, 0], |p| self.screen_position(p));
        let id = self.windows.len();
        self.windows.push(Window {
            id,
            parent,
            children: vec![],
            kind: w.kind.clone(),
            name: w.name().map(str::to_string),
            status: w.status(),
            style: w.style(),
            position: [lo[0] - origin[0], lo[1] - origin[1]],
            size: [hi[0] - lo[0], hi[1] - lo[1]],
            script: wnd::Window { children: vec![], ..w.clone() },
        });
        match parent {
            // addWindowToParent / linkWindow: at the head of the list.
            Some(p) => self.windows[p].children.insert(0, id),
            None => self.top.insert(0, id),
        }
        for c in &w.children {
            self.create(c, Some(id));
        }
        id
    }

    /// `winGetScreenPosition`.
    pub fn screen_position(&self, id: WindowId) -> [i32; 2] {
        let mut p = self.windows[id].position;
        let mut parent = self.windows[id].parent;
        while let Some(q) = parent {
            p[0] += self.windows[q].position[0];
            p[1] += self.windows[q].position[1];
            parent = self.windows[q].parent;
        }
        p
    }

    /// `winGetWindowFromId` by the window's script name.
    pub fn find(&self, name: &str) -> Option<WindowId> {
        self.windows.iter().find(|w| w.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name))).map(|w| w.id)
    }

    pub fn is_hidden(&self, id: WindowId) -> bool {
        self.windows[id].status & status::HIDDEN != 0
    }
    /// `winHide`.
    pub fn hide(&mut self, id: WindowId, hidden: bool) {
        let s = &mut self.windows[id].status;
        if hidden { *s |= status::HIDDEN } else { *s &= !status::HIDDEN }
    }
    /// `winEnable`.
    pub fn enable(&mut self, id: WindowId, enabled: bool) {
        let s = &mut self.windows[id].status;
        if enabled { *s |= status::ENABLED } else { *s &= !status::ENABLED }
    }

    fn contains(&self, id: WindowId, p: [i32; 2]) -> bool {
        let o = self.screen_position(id);
        let s = self.windows[id].size;
        // Edges inclusive, as the original tests.
        p[0] >= o[0] && p[0] <= o[0] + s[0] && p[1] >= o[1] && p[1] <= o[1] + s[1]
    }

    /// `GameWindow::winPointInChild`: the deepest enabled, visible window under
    /// the point, or `id` itself. A disabled child under the point is passed
    /// over, so a sibling below it (or the parent) is found instead.
    pub fn point_in_child(&self, id: WindowId, p: [i32; 2]) -> WindowId {
        for &c in &self.windows[id].children {
            if !self.contains(c, p) || self.is_hidden(c) {
                continue;
            }
            if self.windows[c].status & status::ENABLED != 0 {
                return self.point_in_child(c, p);
            }
        }
        id
    }

    /// The window under a screen point: the topmost visible top-level window that
    /// contains it, then `point_in_child`.
    pub fn window_at(&self, p: [i32; 2]) -> Option<WindowId> {
        self.top.iter().copied().find(|&t| !self.is_hidden(t) && self.contains(t, p)).map(|t| self.point_in_child(t, p))
    }

    /// Every window of a tree, in drawing order (back to front).
    pub fn draw_order(&self) -> Vec<WindowId> {
        fn walk(m: &WindowManager, id: WindowId, out: &mut Vec<WindowId>) {
            if m.is_hidden(id) {
                return;
            }
            out.push(id);
            for &c in m.windows[id].children.iter().rev() {
                walk(m, c, out);
            }
        }
        let mut out = vec![];
        for &t in self.top.iter().rev() {
            walk(self, t, &mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MENU: &str = r#"FILE_VERSION = 2;
WINDOW
  WINDOWTYPE = USER;
  SCREENRECT = UPPERLEFT: 0 0, BOTTOMRIGHT: 800 600, CREATIONRESOLUTION: 800 600;
  NAME = "M.wnd:Parent";
  STATUS = ENABLED;
  CHILD
  WINDOW
    WINDOWTYPE = PUSHBUTTON;
    SCREENRECT = UPPERLEFT: 100 100, BOTTOMRIGHT: 300 140, CREATIONRESOLUTION: 800 600;
    NAME = "M.wnd:Back";
    STATUS = ENABLED;
  END
  WINDOW
    WINDOWTYPE = PUSHBUTTON;
    SCREENRECT = UPPERLEFT: 200 120, BOTTOMRIGHT: 400 160, CREATIONRESOLUTION: 800 600;
    NAME = "M.wnd:Front";
    STATUS = ENABLED;
    CHILD
    WINDOW
      WINDOWTYPE = STATICTEXT;
      SCREENRECT = UPPERLEFT: 210 125, BOTTOMRIGHT: 260 135, CREATIONRESOLUTION: 800 600;
      NAME = "M.wnd:Label";
      STATUS = NOINPUT;
    END
    ENDALLCHILDREN
  END
  ENDALLCHILDREN
END
"#;

    fn manager(display: [i32; 2]) -> WindowManager {
        let mut m = WindowManager::new(display);
        m.load(&wnd::parse(MENU.as_bytes()).unwrap());
        m
    }

    #[test]
    fn rects_scale_to_the_display_and_children_are_parent_relative() {
        let m = manager([1600, 1200]);
        let front = m.find("m.wnd:front").unwrap();
        let label = m.find("M.wnd:Label").unwrap();
        assert_eq!(m.screen_position(front), [400, 240]);
        assert_eq!(m.windows[front].size, [400, 80]);
        assert_eq!(m.windows[label].position, [20, 10], "relative to its parent");
        assert_eq!(m.screen_position(label), [420, 250]);
    }

    #[test]
    fn the_last_child_in_the_file_is_on_top() {
        let mut m = manager([800, 600]);
        let (back, front) = (m.find("M.wnd:Back").unwrap(), m.find("M.wnd:Front").unwrap());
        // Both cover (250, 130): the later one wins.
        assert_eq!(m.window_at([250, 130]), Some(front));
        assert_eq!(m.window_at([150, 110]), Some(back));
        // The label is not enabled, so the button under it is found.
        assert_eq!(m.window_at([220, 128]), Some(front));
        // Hidden or disabled, the front button lets the back one through.
        m.hide(front, true);
        assert_eq!(m.window_at([250, 130]), Some(back));
        m.hide(front, false);
        m.enable(front, false);
        assert_eq!(m.window_at([250, 130]), Some(back));
        // Outside every child: the parent.
        assert_eq!(m.window_at([700, 500]), m.find("M.wnd:Parent"));
        let order: Vec<_> = m.draw_order().iter().map(|&i| m.windows[i].name.clone().unwrap()).collect();
        assert_eq!(order, ["M.wnd:Parent", "M.wnd:Back", "M.wnd:Front", "M.wnd:Label"]);
    }
}
