// Windows release builds should not open a console behind the window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    excalibur_lib::run();
}
