use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use super::{data::PanelOrder, position::PanelPosition};
use crate::config::icon::UmideIcons;

#[derive(
    Clone, Copy, PartialEq, Serialize, Deserialize, Hash, Eq, Debug, EnumIter,
)]
pub enum PanelKind {
    Terminal,
    FileExplorer,
    SourceControl,
    Plugin,
    Search,
    Problem,
    Debug,
    CallHierarchy,
    DocumentSymbol,
    References,
    Implementation,
    Emulator,
    Video,
}

impl PanelKind {
    pub fn svg_name(&self) -> &'static str {
        match &self {
            PanelKind::Terminal => UmideIcons::TERMINAL,
            PanelKind::FileExplorer => UmideIcons::FILE_EXPLORER,
            PanelKind::SourceControl => UmideIcons::SCM,
            PanelKind::Plugin => UmideIcons::EXTENSIONS,
            PanelKind::Search => UmideIcons::SEARCH,
            PanelKind::Problem => UmideIcons::PROBLEM,
            PanelKind::Debug => UmideIcons::DEBUG,
            PanelKind::CallHierarchy => UmideIcons::TYPE_HIERARCHY,
            PanelKind::DocumentSymbol => UmideIcons::DOCUMENT_SYMBOL,
            PanelKind::References => UmideIcons::REFERENCES,
            PanelKind::Implementation => UmideIcons::IMPLEMENTATION,
            PanelKind::Emulator => UmideIcons::EMULATOR,
            PanelKind::Video => UmideIcons::DEBUG_CONSOLE,
        }
    }

    pub fn position(&self, order: &PanelOrder) -> Option<(usize, PanelPosition)> {
        for (pos, panels) in order.iter() {
            let index = panels.iter().position(|k| k == self);
            if let Some(index) = index {
                return Some((index, *pos));
            }
        }
        None
    }

    pub fn default_position(&self) -> PanelPosition {
        match self {
            PanelKind::Terminal => PanelPosition::BottomLeft,
            PanelKind::FileExplorer => PanelPosition::LeftTop,
            PanelKind::SourceControl => PanelPosition::LeftTop,
            PanelKind::Plugin => PanelPosition::LeftTop,
            PanelKind::Search => PanelPosition::BottomLeft,
            PanelKind::Problem => PanelPosition::BottomLeft,
            PanelKind::Debug => PanelPosition::LeftTop,
            PanelKind::CallHierarchy => PanelPosition::BottomLeft,
            PanelKind::DocumentSymbol => PanelPosition::RightTop,
            PanelKind::References => PanelPosition::BottomLeft,
            PanelKind::Implementation => PanelPosition::BottomLeft,
            PanelKind::Emulator => PanelPosition::RightTop,
            PanelKind::Video => PanelPosition::RightBottom,
        }
    }
}
