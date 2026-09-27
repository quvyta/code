//! The tab strip and the rail as the keyboard walks them.
//!
//! Switching to a tab puts the keyboard in it, so the person types into the harness they just
//! chose without clicking it first. Walking the strip with its arrows is the one switch that must
//! not: every arrow would carry the keyboard off the strip, and the next arrow would go to the
//! harness. [`Walked`] tells the screen when the strip or the rail starts and stops handling a key,
//! so a switch in between is known as the keyboard walking there; and it takes the keys that step
//! into the open tab, which the strip itself leaves unused.

use qframe::event::KeyKind;
use qframe::keymap::Key;
use qframe::prelude::*;
use qframe::widget::{EventCx, MeasureCx, Node, PaintCx, Widget};

use super::Msg;

/// The strip or the rail, saying so when the keyboard walks it and stepping into the open tab on
/// the keys in `enter`.
pub(super) struct Walked<W> {
    inner: W,
    enter: &'static [Key],
}

impl<W> Walked<W> {
    /// `inner`, whose keys step into the open tab when it leaves them unused.
    pub(super) fn new(inner: W, enter: &'static [Key]) -> Self {
        Self { inner, enter }
    }
}

impl<W: Widget<Msg>> Widget<Msg> for Walked<W> {
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
        let Event::Key(key) = event else { return self.inner.event(cx, event) };
        if key.kind == KeyKind::Release {
            return self.inner.event(cx, event);
        }
        // Around what the strip answers, so a switch it sends arrives between the two, in order.
        cx.emit(Msg::Walking(true));
        let used = self.inner.event(cx, event);
        cx.emit(Msg::Walking(false));
        if used {
            return true;
        }
        if self.enter.iter().any(|&enter| key.is_plain(enter)) {
            cx.emit(Msg::EnterTab);
            return true;
        }
        false
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
