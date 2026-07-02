use std::{rc::Rc, sync::Arc};

use floem::{
    kurbo::Size,
    reactive::{
        Context, Memo, RwSignal, Scope, SignalGet, SignalUpdate, SignalWith,
    },
};
use serde::{Deserialize, Serialize};

use super::{
    kind::PanelKind,
    position::{PanelContainerPosition, PanelPosition},
    style::PanelStyle,
};
use crate::{
    db::UmideDb,
    window_tab::{CommonData, Focus},
};

pub type PanelOrder = im::HashMap<PanelPosition, im::Vector<PanelKind>>;

pub fn default_panel_order() -> PanelOrder {
    let mut order = PanelOrder::new();
    order.insert(
        PanelPosition::LeftTop,
        im::vector![
            PanelKind::FileExplorer,
            PanelKind::Plugin,
            PanelKind::SourceControl,
            PanelKind::Debug,
        ],
    );
    order.insert(
        PanelPosition::BottomLeft,
        im::vector![
            PanelKind::Terminal,
            PanelKind::Search,
            PanelKind::Problem,
            PanelKind::CallHierarchy,
            PanelKind::References,
            PanelKind::Implementation
        ],
    );
    // Mobile-first layout: the emulator owns the WHOLE right column (a phone
    // screen is tall — a fresh workspace opens SHOWING the device at full
    // height), and the AI assistant sits in the bottom dock beside the
    // terminal, where a chat gets real width. Everything is visible at once —
    // watch the agent drive the emulator — and each panel closes
    // independently. DocumentSymbol is deliberately not docked by default
    // (open it on demand); it isn't part of the mobile core loop.
    order.insert(PanelPosition::RightTop, im::vector![PanelKind::Emulator]);
    order.insert(PanelPosition::RightBottom, im::vector![PanelKind::Video]);
    order.insert(PanelPosition::LeftBottom, im::vector![PanelKind::Terminal]);
    order.insert(
        PanelPosition::BottomRight,
        im::vector![PanelKind::AiAssistant],
    );

    order
}

/// One-time layout migration for saved orders that still carry a PREVIOUS
/// default layout:
///   * pre-v1: `[DocumentSymbol, Emulator, AiAssistant]` packed into ONE tab
///     area (RightTop) — emulator and agent were mutually exclusive;
///   * v1: `[Emulator, DocumentSymbol]` up top — before DocumentSymbol was
///     undocked;
///   * v2: emulator alone up top with the assistant docked directly below it —
///     which split the right column and starved both (the device view lost the
///     height a phone screen needs, the chat lost width).
/// Rewrites to the current default: emulator alone owning the right column,
/// assistant in the bottom dock beside the terminal. A customized order matches
/// none of the old defaults, so user-arranged layouts are left untouched.
/// Returns whether it migrated, so the caller can also fix section visibility
/// once (see the `PANEL_LAYOUT_VERSION` check at load).
pub fn migrate_dock_layout(order: &mut PanelOrder) -> bool {
    let top = order.get(&PanelPosition::RightTop);
    let from_old_top = [
        im::vector![
            PanelKind::DocumentSymbol,
            PanelKind::Emulator,
            PanelKind::AiAssistant
        ],
        im::vector![PanelKind::Emulator, PanelKind::DocumentSymbol],
    ]
    .iter()
    .any(|d| top == Some(d));
    // The v2 interim: emulator alone up top + assistant heading RightBottom.
    let from_v2 = top == Some(&im::vector![PanelKind::Emulator])
        && order
            .get(&PanelPosition::RightBottom)
            .is_some_and(|b| b.front() == Some(&PanelKind::AiAssistant));
    if !from_old_top && !from_v2 {
        return false;
    }
    order.insert(PanelPosition::RightTop, im::vector![PanelKind::Emulator]);
    if let Some(rb) = order.get_mut(&PanelPosition::RightBottom) {
        rb.retain(|k| k != &PanelKind::AiAssistant);
    }
    let bottom_right = order.entry(PanelPosition::BottomRight).or_default();
    if !bottom_right.contains(&PanelKind::AiAssistant) {
        bottom_right.push_front(PanelKind::AiAssistant);
    }
    true
}

#[derive(Clone, Copy, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub enum PanelSection {
    OpenEditor,
    FileExplorer,
    Error,
    Warn,
    Changes,
    Installed,
    Available,
    Process,
    Variable,
    StackFrame,
    Breakpoint,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PanelSize {
    pub left: f64,
    pub left_split: f64,
    pub bottom: f64,
    pub bottom_split: f64,
    pub right: f64,
    pub right_split: f64,
}

/// Bump when the default layout changes shape and saved styles/orders need a
/// one-time rewrite (see `migrate_dock_layout` + the version check at load).
/// Saved data predating the field deserializes as 0.
/// v1: assistant moved to RightBottom. v2: DocumentSymbol undocked.
/// v3: assistant moved to the bottom dock; emulator owns the right column.
pub const PANEL_LAYOUT_VERSION: u32 = 3;

#[derive(Clone, Serialize, Deserialize)]
pub struct PanelInfo {
    pub panels: PanelOrder,
    pub styles: im::HashMap<PanelPosition, PanelStyle>,
    pub size: PanelSize,
    pub sections: im::HashMap<PanelSection, bool>,
    #[serde(default)]
    pub version: u32,
}

#[derive(Clone)]
pub struct PanelData {
    pub panels: RwSignal<PanelOrder>,
    pub styles: RwSignal<im::HashMap<PanelPosition, PanelStyle>>,
    pub size: RwSignal<PanelSize>,
    pub available_size: Memo<Size>,
    pub sections: RwSignal<im::HashMap<PanelSection, RwSignal<bool>>>,
    pub common: Rc<CommonData>,
    pub android_frame: RwSignal<Option<Arc<umide_emulator::decoder::DecodedFrame>>>,
    pub ios_frame: RwSignal<Option<Arc<umide_emulator::decoder::DecodedFrame>>>,
    /// The emulator/simulator device the user currently has running and shown in
    /// the Emulator panel, or `None`. Producer: the Emulator panel (mirrors its
    /// running/visible device). Consumer: the AI agent's `resolve_target`, so a
    /// turn drives the device the user is viewing instead of "first adb device"
    /// (cross-machine ask G2). `DeviceInfo` carries `.id` (AVD/UDID) + `.platform`.
    pub active_device: RwSignal<Option<umide_emulator::DeviceInfo>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PanelDataInfo {
    pub panels: PanelOrder,
    pub styles: im::HashMap<PanelPosition, PanelStyle>,
    pub size: PanelSize,
    pub sections: im::HashMap<PanelSection, bool>,
}

impl PanelData {
    pub fn new(
        cx: Scope,
        info: PanelDataInfo,
        available_size: Memo<Size>,
        common: Rc<CommonData>,
    ) -> Self {
        let panels = cx.create_rw_signal(info.panels);
        let styles = cx.create_rw_signal(info.styles);
        let size = cx.create_rw_signal(info.size);
        let sections = cx.create_rw_signal(
            info.sections
                .into_iter()
                .map(|(key, value)| (key, cx.create_rw_signal(value)))
                .collect(),
        );
        let android_frame = cx.create_rw_signal(None);
        let ios_frame = cx.create_rw_signal(None);
        let active_device = cx.create_rw_signal(None);

        Self {
            panels,
            styles,
            size,
            available_size,
            sections,
            common,
            android_frame,
            ios_frame,
            active_device,
        }
    }

    pub fn panel_info(&self) -> PanelInfo {
        PanelInfo {
            panels: self.panels.get_untracked(),
            styles: self.styles.get_untracked(),
            size: self.size.get_untracked(),
            sections: self
                .sections
                .get_untracked()
                .into_iter()
                .map(|(key, value)| (key, value.get_untracked()))
                .collect(),
            version: PANEL_LAYOUT_VERSION,
        }
    }

    pub fn is_container_shown(
        &self,
        position: &PanelContainerPosition,
        tracked: bool,
    ) -> bool {
        self.is_position_shown(&position.first(), tracked)
            || self.is_position_shown(&position.second(), tracked)
    }

    pub fn is_position_empty(
        &self,
        position: &PanelPosition,
        tracked: bool,
    ) -> bool {
        if tracked {
            self.panels
                .with(|panels| panels.get(position).map(|p| p.is_empty()))
                .unwrap_or(true)
        } else {
            self.panels
                .with_untracked(|panels| panels.get(position).map(|p| p.is_empty()))
                .unwrap_or(true)
        }
    }

    pub fn is_position_shown(
        &self,
        position: &PanelPosition,
        tracked: bool,
    ) -> bool {
        let styles = if tracked {
            self.styles.get()
        } else {
            self.styles.get_untracked()
        };
        styles.get(position).map(|s| s.shown).unwrap_or(false)
    }

    pub fn panel_position(
        &self,
        kind: &PanelKind,
    ) -> Option<(usize, PanelPosition)> {
        self.panels
            .with_untracked(|panels| panel_position(panels, kind))
    }

    pub fn is_panel_visible(&self, kind: &PanelKind) -> bool {
        if let Some((index, position)) = self.panel_position(kind) {
            if let Some(style) = self
                .styles
                .with_untracked(|styles| styles.get(&position).cloned())
            {
                return style.active == index && style.shown;
            }
        }
        false
    }

    pub fn show_panel(&self, kind: &PanelKind) {
        if let Some((index, position)) = self.panel_position(kind) {
            self.styles.update(|styles| {
                if let Some(style) = styles.get_mut(&position) {
                    style.shown = true;
                    style.active = index;
                }
            });
        }
    }

    pub fn hide_panel(&self, kind: &PanelKind) {
        if let Some((_, position)) = self.panel_position(kind) {
            if let Some((active_panel, _)) =
                self.active_panel_at_position(&position, false)
            {
                if &active_panel == kind {
                    self.set_shown(&position, false);
                    let peer_position = position.peer();
                    if let Some(order) = self
                        .panels
                        .with_untracked(|panels| panels.get(&peer_position).cloned())
                    {
                        if order.is_empty() {
                            self.set_shown(&peer_position, false);
                        }
                    }
                }
            }
        }
    }

    /// Get the active panel kind at that position, if any.
    /// `tracked` decides whether it should track the signal or not.
    pub fn active_panel_at_position(
        &self,
        position: &PanelPosition,
        tracked: bool,
    ) -> Option<(PanelKind, bool)> {
        let style = if tracked {
            self.styles.with(|styles| styles.get(position).cloned())?
        } else {
            self.styles
                .with_untracked(|styles| styles.get(position).cloned())?
        };
        let order = if tracked {
            self.panels.with(|panels| panels.get(position).cloned())?
        } else {
            self.panels
                .with_untracked(|panels| panels.get(position).cloned())?
        };
        order
            .get(style.active)
            .cloned()
            .or_else(|| order.last().cloned())
            .map(|p| (p, style.shown))
    }

    pub fn set_shown(&self, position: &PanelPosition, shown: bool) {
        self.styles.update(|styles| {
            if let Some(style) = styles.get_mut(position) {
                style.shown = shown;
            }
        });
    }

    pub fn toggle_active_maximize(&self) {
        let focus = self.common.focus.get_untracked();
        if let Focus::Panel(kind) = focus {
            if let Some((_, pos)) = self.panel_position(&kind) {
                if pos.is_bottom() {
                    self.toggle_bottom_maximize();
                }
            }
        }
    }

    pub fn toggle_maximize(&self, kind: &PanelKind) {
        if let Some((_, p)) = self.panel_position(kind) {
            if p.is_bottom() {
                self.toggle_bottom_maximize();
            }
        }
    }

    pub fn toggle_bottom_maximize(&self) {
        let maximized = !self.panel_bottom_maximized(false);
        self.styles.update(|styles| {
            if let Some(style) = styles.get_mut(&PanelPosition::BottomLeft) {
                style.maximized = maximized;
            }
            if let Some(style) = styles.get_mut(&PanelPosition::BottomRight) {
                style.maximized = maximized;
            }
        });
    }

    pub fn panel_bottom_maximized(&self, tracked: bool) -> bool {
        let styles = if tracked {
            self.styles.get()
        } else {
            self.styles.get_untracked()
        };
        styles
            .get(&PanelPosition::BottomLeft)
            .map(|p| p.maximized)
            .unwrap_or(false)
            || styles
                .get(&PanelPosition::BottomRight)
                .map(|p| p.maximized)
                .unwrap_or(false)
    }

    pub fn toggle_container_visual(&self, position: &PanelContainerPosition) {
        let is_hidden = !self.is_container_shown(position, false);
        if is_hidden {
            self.styles.update(|styles| {
                let style = styles.entry(position.first()).or_default();
                style.shown = true;
                let style = styles.entry(position.second()).or_default();
                style.shown = true;
            });
        } else {
            if let Some((kind, _)) =
                self.active_panel_at_position(&position.second(), false)
            {
                self.hide_panel(&kind);
            }
            if let Some((kind, _)) =
                self.active_panel_at_position(&position.first(), false)
            {
                self.hide_panel(&kind);
            }
            self.styles.update(|styles| {
                let style = styles.entry(position.first()).or_default();
                style.shown = false;
                let style = styles.entry(position.second()).or_default();
                style.shown = false;
            });
        }
    }

    pub fn move_panel_to_position(&self, kind: PanelKind, position: &PanelPosition) {
        let current_position = self.panel_position(&kind);
        if current_position.as_ref().map(|(_, pos)| pos) == Some(position) {
            return;
        }

        let mut new_index_at_old_position = None;
        let index = self
            .panels
            .try_update(|panels| {
                if let Some((index, current_position)) = current_position {
                    if let Some(panels) = panels.get_mut(&current_position) {
                        panels.remove(index);

                        let max_index = panels.len().saturating_sub(1);
                        if index > max_index {
                            new_index_at_old_position = Some(max_index);
                        }
                    }
                }
                let panels = panels.entry(*position).or_default();
                panels.push_back(kind);
                panels.len() - 1
            })
            .unwrap();
        self.styles.update(|styles| {
            if let Some((_, current_position)) = current_position {
                if let Some(new_index) = new_index_at_old_position {
                    let style = styles.entry(current_position).or_default();
                    style.active = new_index;
                }
            }

            let style = styles.entry(*position).or_default();
            style.active = index;
            style.shown = true;
        });

        let db = crate::app::get_db();
        db.save_panel_orders(self.panels.get_untracked());
    }

    pub fn section_open(&self, section: PanelSection) -> RwSignal<bool> {
        let open = self
            .sections
            .with_untracked(|sections| sections.get(&section).cloned());
        if let Some(open) = open {
            return open;
        }

        let open = self.common.scope.create_rw_signal(true);
        self.sections.update(|sections| {
            sections.insert(section, open);
        });
        open
    }
}

pub fn panel_position(
    order: &PanelOrder,
    kind: &PanelKind,
) -> Option<(usize, PanelPosition)> {
    for (pos, panels) in order.iter() {
        let index = panels.iter().position(|k| k == kind);
        if let Some(index) = index {
            return Some((index, *pos));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The v3 target: emulator alone owns the right column, assistant heads
    /// the bottom dock's right section, nothing left in RightBottom but Video.
    fn assert_v3_shape(order: &PanelOrder) {
        assert_eq!(
            order.get(&PanelPosition::RightTop),
            Some(&im::vector![PanelKind::Emulator])
        );
        assert_eq!(
            order.get(&PanelPosition::RightBottom),
            Some(&im::vector![PanelKind::Video])
        );
        assert_eq!(
            order
                .get(&PanelPosition::BottomRight)
                .and_then(|v| v.front()),
            Some(&PanelKind::AiAssistant)
        );
    }

    #[test]
    fn migrate_dock_layout_rewrites_every_old_default() {
        // Pre-v1: all three packed into RightTop.
        let mut order = PanelOrder::new();
        order.insert(
            PanelPosition::RightTop,
            im::vector![
                PanelKind::DocumentSymbol,
                PanelKind::Emulator,
                PanelKind::AiAssistant
            ],
        );
        order.insert(PanelPosition::RightBottom, im::vector![PanelKind::Video]);
        assert!(migrate_dock_layout(&mut order));
        assert_v3_shape(&order);
        // Running it again is a no-op (no longer matches any old default).
        assert!(!migrate_dock_layout(&mut order));

        // v1: [Emulator, DocumentSymbol] up top.
        let mut v1 = PanelOrder::new();
        v1.insert(
            PanelPosition::RightTop,
            im::vector![PanelKind::Emulator, PanelKind::DocumentSymbol],
        );
        v1.insert(PanelPosition::RightBottom, im::vector![PanelKind::Video]);
        assert!(migrate_dock_layout(&mut v1));
        assert_v3_shape(&v1);

        // v2: emulator alone up top, assistant docked below it.
        let mut v2 = PanelOrder::new();
        v2.insert(PanelPosition::RightTop, im::vector![PanelKind::Emulator]);
        v2.insert(
            PanelPosition::RightBottom,
            im::vector![PanelKind::AiAssistant, PanelKind::Video],
        );
        assert!(migrate_dock_layout(&mut v2));
        assert_v3_shape(&v2);
    }

    #[test]
    fn migrate_dock_layout_leaves_customized_layouts_alone() {
        // The user moved the assistant somewhere deliberate → untouched.
        let mut order = PanelOrder::new();
        order.insert(
            PanelPosition::RightTop,
            im::vector![PanelKind::AiAssistant, PanelKind::Emulator],
        );
        let before = order.clone();
        assert!(!migrate_dock_layout(&mut order));
        assert_eq!(order, before);
    }

    #[test]
    fn fresh_default_is_the_v3_shape() {
        assert_v3_shape(&default_panel_order());
    }
}
