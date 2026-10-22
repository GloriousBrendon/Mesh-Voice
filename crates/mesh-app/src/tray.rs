//! Windows: Mesh's icon in the notification area. Closing the window hides
//! Mesh there, so calls and notifications keep going; clicking the icon
//! brings it back, and its menu quits. Elsewhere there's no tray and closing
//! quits as before.

use crate::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Show,
    Quit,
}

#[cfg(windows)]
thread_local! {
    /// Kept alive for the life of the app: dropping it removes the icon.
    static TRAY: std::cell::RefCell<Option<tray_icon::TrayIcon>> = const { std::cell::RefCell::new(None) };
}

/// Puts the icon up (on the UI thread, whose event loop it rides on).
/// Whether there is one.
pub fn create() -> bool {
    #[cfg(windows)]
    {
        use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
        let Ok(png) = image::load_from_memory(include_bytes!("../assets/mesh.png")) else { return false };
        let rgba = image::imageops::resize(&png.into_rgba8(), 32, 32, image::imageops::FilterType::Lanczos3);
        let Ok(icon) = tray_icon::Icon::from_rgba(rgba.into_raw(), 32, 32) else { return false };
        let menu = Menu::new();
        let built = menu
            .append_items(&[
                &MenuItem::with_id("open", "Open Mesh", true, None),
                &PredefinedMenuItem::separator(),
                &MenuItem::with_id("quit", "Quit Mesh", true, None),
            ])
            .map_err(|e| e.to_string())
            .and_then(|()| {
                tray_icon::TrayIconBuilder::new()
                    .with_icon(icon)
                    .with_tooltip("Mesh")
                    .with_menu(Box::new(menu))
                    .with_menu_on_left_click(false)
                    .build()
                    .map_err(|e| e.to_string())
            });
        match built {
            Ok(tray) => {
                TRAY.with(|t| *t.borrow_mut() = Some(tray));
                true
            }
            Err(err) => {
                tracing::warn!("no tray icon: {err}");
                false
            }
        }
    }
    #[cfg(not(windows))]
    false
}

/// Clicks on the icon and its menu, as messages.
pub fn events() -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(16, async move |mut output: iced::futures::channel::mpsc::Sender<Message>| {
        use iced::futures::SinkExt;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        #[cfg(windows)]
        {
            use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent, menu::MenuEvent};
            let clicks = tx.clone();
            TrayIconEvent::set_event_handler(Some(move |event| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
                | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } = event
                {
                    let _ = clicks.send(Action::Show);
                }
            }));
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let _ = tx.send(if event.id == "quit" { Action::Quit } else { Action::Show });
            }));
        }
        #[cfg(not(windows))]
        drop(tx);
        while let Some(action) = rx.recv().await {
            if output.send(Message::Tray(action)).await.is_err() {
                break;
            }
        }
    })
}
