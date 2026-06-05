// Windows: 防止多余 console 窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    musage_lib::run();
}
