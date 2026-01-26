#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use umide_app::app;

pub fn main() {
    app::launch();
}
