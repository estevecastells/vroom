//! The tray icon, its menu and the global shortcut (Alt+Shift+R, as ⌥⇧R on the
//! Mac). Their events arrive on other threads; they are forwarded to the app
//! with a repaint request, which wakes eframe even while the window is hidden.

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::sync::mpsc::Sender;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use vroom_core::i18n::t;

pub enum TrayMsg {
    Toggle,
    Cancel,
    Show,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    _hotkeys: Option<GlobalHotKeyManager>,
    record: MenuItem,
    cancel: MenuItem,
    show: MenuItem,
    quit: MenuItem,
}

impl Tray {
    /// Must run on the thread with the event loop (eframe's creation callback).
    pub fn new(ctx: eframe::egui::Context, tx: Sender<TrayMsg>) -> Result<Tray, String> {
        // Same texts as `relabel`, which sets them again when the language changes.
        let record = MenuItem::with_id("record", t("Grabar\tAlt+Mayús+R"), true, None);
        let cancel = MenuItem::with_id("cancel", t("Cancelar grabación"), false, None);
        let show = MenuItem::with_id("show", t("Abrir biblioteca"), true, None);
        let quit = MenuItem::with_id("quit", t("Salir de Vroom"), true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &record,
            &cancel,
            &PredefinedMenuItem::separator(),
            &show,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| e.to_string())?;
        let rgba = icon_rgba(32);
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("Vroom")
            .with_icon(tray_icon::Icon::from_rgba(rgba, 32, 32).map_err(|e| e.to_string())?)
            .build()
            .map_err(|e| e.to_string())?;

        let (menu_tx, menu_ctx) = (tx.clone(), ctx.clone());
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let msg = match e.id.as_ref() {
                "record" => TrayMsg::Toggle,
                "cancel" => TrayMsg::Cancel,
                "show" => TrayMsg::Show,
                "quit" => TrayMsg::Quit,
                _ => return,
            };
            let _ = menu_tx.send(msg);
            menu_ctx.request_repaint();
        }));
        let (click_tx, click_ctx) = (tx.clone(), ctx.clone());
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                let _ = click_tx.send(TrayMsg::Show);
                click_ctx.request_repaint();
            }
        }));

        // Another app may own the shortcut; the menu still works without it.
        let hotkeys = GlobalHotKeyManager::new()
            .ok()
            .filter(|m| m.register(HotKey::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::KeyR)).is_ok());
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state == HotKeyState::Pressed {
                let _ = tx.send(TrayMsg::Toggle);
                ctx.request_repaint();
            }
        }));
        Ok(Tray { _icon: icon, _hotkeys: hotkeys, record, cancel, show, quit })
    }

    pub fn set_recording(&self, recording: bool) {
        self.record.set_text(if recording { t("Detener grabación\tAlt+Mayús+R") } else { t("Grabar\tAlt+Mayús+R") });
        self.cancel.set_enabled(recording);
    }

    /// Sets every menu text again, after the language changed.
    pub fn relabel(&self, recording: bool) {
        self.set_recording(recording);
        self.cancel.set_text(t("Cancelar grabación"));
        self.show.set_text(t("Abrir biblioteca"));
        self.quit.set_text(t("Salir de Vroom"));
    }
}

/// The app icon drawn in code, like scripts/make-icon.swift draws the Mac one: a
/// warm rounded square with a white ring and a red dot.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded square with a 22% corner radius.
            let r = s * 0.22;
            let dx = (fx - fx.clamp(r, s - r)).abs();
            let dy = (fy - fy.clamp(r, s - r)).abs();
            let inside = (dx * dx + dy * dy).sqrt() <= r;
            if !inside {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            // Orange (top left) to pink (bottom right).
            let t = (fx + fy) / (2.0 * s);
            let mut px = [(255.0 - 36.0 * t) as u8, (120.0 - 82.0 * t) as u8, (74.0 + 23.0 * t) as u8, 255];
            let d = ((fx - s / 2.0).powi(2) + (fy - s / 2.0).powi(2)).sqrt();
            if d <= s * 0.18 {
                px = [237, 46, 69, 255];
            } else if d <= s * 0.30 {
                px = [255, 255, 255, 255];
            }
            out.extend_from_slice(&px);
        }
    }
    out
}
