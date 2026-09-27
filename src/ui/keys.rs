//! What the keyboard does on QCode's lists and strips beyond what the framework's widgets do by
//! themselves, laid over those widgets rather than copied from them.
//!
//! A person who reaches the last row of a menu and presses Down expects the first row, as in the
//! menus of every desktop; the framework's lists stop at their ends and keep the key. Until they
//! can be told to go round, [`Wrapping`] turns the key that would go past an end into the key that goes to the other one,
//! Home or End, which every list and radio group already answers. Nothing else of the widget is
//! touched: it measures, paints and handles every other event itself.

use qframe::event::KeyKind;
use qframe::keymap::{Key, KeyChord, Modifiers};
use qframe::prelude::*;
use qframe::widget::{EventCx, MeasureCx, Node, PaintCx, Widget};

/// A list, radio group or anything else that answers the arrows, Home and End, going round at its
/// ends: Down (or Right, laid out across) on the last row goes to the first, Up (or Left) on the
/// first goes to the last.
pub struct Wrapping<W> {
    inner: W,
    first: bool,
    last: bool,
    across: bool,
}

impl<W> Wrapping<W> {
    /// `inner`, whose highlighted row is `at` of `rows` rows that can all be chosen. A list with
    /// rows that cannot be chosen, such as headings, says where its ends are with
    /// [`edges`](Self::edges) instead.
    #[must_use]
    pub fn new(inner: W, at: Option<usize>, rows: usize) -> Self {
        let first = at == Some(0);
        let last = rows > 0 && at == Some(rows - 1);
        Self { inner, first, last, across: false }
    }

    /// Whether the highlighted row is the first and whether it is the last of the rows that can
    /// be chosen.
    #[must_use]
    pub fn edges(mut self, first: bool, last: bool) -> Self {
        self.first = first;
        self.last = last;
        self
    }

    /// The rows are laid out across, as in a horizontal radio group: Left and Right go round.
    #[must_use]
    pub fn across(mut self, across: bool) -> Self {
        self.across = across;
        self
    }

    /// The key to press instead of `key`: the one that goes to the other end, when `key` would go
    /// past this one.
    fn round(&self, key: &KeyEvent) -> Option<Key> {
        let (back, forward) = if self.across { (Key::Left, Key::Right) } else { (Key::Up, Key::Down) };
        if self.last && key.is_plain(forward) {
            Some(Key::Home)
        } else if self.first && key.is_plain(back) {
            Some(Key::End)
        } else {
            None
        }
    }
}

impl<W: Widget<Msg>, Msg: 'static> Widget<Msg> for Wrapping<W> {
    fn measure(&self, cx: &mut MeasureCx<'_>, available: Size) -> Size {
        self.inner.measure(cx, available)
    }

    fn paint(&self, cx: &mut PaintCx<'_>, area: Rect) {
        self.inner.paint(cx, area);
    }

    fn paint_overlay(&self, cx: &mut PaintCx<'_>, anchor: Rect) {
        self.inner.paint_overlay(cx, anchor);
    }

    fn event(&self, cx: &mut EventCx<'_, Msg>, event: &Event) -> bool {
        if let Event::Key(key) = event
            && key.kind != KeyKind::Release
            && let Some(other) = self.round(key)
        {
            let chord = KeyChord { key: other, mods: Modifiers::default() };
            let pressed = KeyEvent { chord, kind: key.kind, text: None };
            return self.inner.event(cx, &Event::Key(pressed));
        }
        self.inner.event(cx, event)
    }

    fn focusable(&self) -> bool {
        self.inner.focusable()
    }

    fn children(&self) -> &[Node<Msg>] {
        self.inner.children()
    }

    fn children_mut(&mut self) -> &mut [Node<Msg>] {
        self.inner.children_mut()
    }
}
