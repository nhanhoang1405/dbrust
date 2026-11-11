use anyhow::{Result, bail};

#[derive(Default)]
pub struct TextClipboard {
    system: Option<arboard::Clipboard>,
    internal: Option<String>,
}
impl TextClipboard {
    fn connect(&mut self) {
        if self.system.is_none()
            && !cfg!(test)
            && std::env::var("DBRUST_CLIPBOARD").as_deref() != Ok("internal")
        {
            self.system = arboard::Clipboard::new().ok();
        }
    }
    /// Keep the provider alive: on Linux the application owns copied content.
    pub fn copy(&mut self, text: String) -> bool {
        self.connect();
        self.internal = Some(text.clone());
        self.system
            .as_mut()
            .is_some_and(|clipboard| clipboard.set_text(text).is_ok())
    }
    pub fn paste(&mut self) -> Result<String> {
        self.connect();
        if let Some(system) = &mut self.system
            && let Ok(text) = system.get_text()
        {
            return Ok(text);
        }
        if let Some(text) = &self.internal {
            return Ok(text.clone());
        }
        bail!(
            "Clipboard unavailable or empty. Copy inside dbrust first, or use your terminal's paste shortcut"
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headless_clipboard_preserves_multiline_unicode_and_empty_text() {
        let mut c = TextClipboard::default();
        assert!(c.paste().is_err());
        assert!(!c.copy("hello 🌍\nline two".into()));
        assert_eq!(c.paste().unwrap(), "hello 🌍\nline two");
        c.copy(String::new());
        assert_eq!(c.paste().unwrap(), "");
    }
}
