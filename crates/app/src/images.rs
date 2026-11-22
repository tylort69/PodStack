//! EVE image server pictures (portraits, logos, type icons), cached in memory and on disk.

use crate::rt;
use gtk::{gdk, glib, prelude::*};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

thread_local! {
    static MEMORY: RefCell<HashMap<String, gdk::Texture>> = RefCell::new(HashMap::new());
    static WAITING: RefCell<HashMap<String, Vec<glib::WeakRef<gtk::Image>>>> = RefCell::new(HashMap::new());
}

fn disk_path(url: &str) -> std::path::PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    podstack_core::store::cache_dir().join("images").join(format!("{:016x}", h.finish()))
}

/// Shows the image at `url` in `picture` once loaded (immediately when cached).
pub fn load_into(picture: &gtk::Image, url: &str) {
    if let Some(t) = MEMORY.with(|m| m.borrow().get(url).cloned()) {
        picture.set_paintable(Some(&t));
        return;
    }
    let first = WAITING.with(|w| {
        let mut w = w.borrow_mut();
        let list = w.entry(url.to_string()).or_default();
        list.push(picture.downgrade());
        list.len() == 1
    });
    if !first {
        return;
    }
    let url_s = url.to_string();
    rt::run(
        async move {
            let path = disk_path(&url_s);
            if let Ok(bytes) = tokio::fs::read(&path).await {
                return Some(bytes);
            }
            let res = reqwest::Client::new()
                .get(&url_s)
                .header("User-Agent", podstack_core::config::user_agent())
                .send()
                .await
                .ok()?;
            if !res.status().is_success() {
                return None;
            }
            let bytes = res.bytes().await.ok()?.to_vec();
            let _ = tokio::fs::create_dir_all(path.parent().unwrap()).await;
            let _ = tokio::fs::write(&path, &bytes).await;
            Some(bytes)
        },
        {
            let url = url.to_string();
            move |bytes| {
                let texture = bytes.and_then(|b| gdk::Texture::from_bytes(&glib::Bytes::from_owned(b)).ok());
                let waiting = WAITING.with(|w| w.borrow_mut().remove(&url)).unwrap_or_default();
                if let Some(t) = texture {
                    MEMORY.with(|m| m.borrow_mut().insert(url.clone(), t.clone()));
                    for p in waiting.iter().filter_map(|w| w.upgrade()) {
                        p.set_paintable(Some(&t));
                    }
                }
            }
        },
    );
}

/// A `size`×`size` image loading `url` (blank until loaded).
pub fn image(url: &str, size: i32) -> gtk::Image {
    let img = gtk::Image::new();
    img.set_pixel_size(size);
    img.set_size_request(size, size);
    load_into(&img, url);
    img
}

/// A round portrait / logo.
pub fn avatar(url: &str, size: i32) -> gtk::Widget {
    let img = image(url, size);
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("avatar");
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.set_halign(gtk::Align::Center);
    frame.set_valign(gtk::Align::Center);
    frame.append(&img);
    frame.upcast()
}
