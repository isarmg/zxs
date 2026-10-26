//! Startup has one terminal outcome. Resource completion cannot revive a failed page.
pub const TIMEOUT_MS: u32 = 30_000;
pub const READY_EVENT: &str = "zxs-startup-ready";
pub const FAILED_EVENT: &str = "zxs-startup-failed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Pending,
    Ready,
    Failed,
}
impl Phase {
    pub fn from_attribute(value: Option<&str>) -> Option<Self> {
        match value {
            None | Some("pending") => Some(Self::Pending),
            Some("ready") => Some(Self::Ready),
            Some("failed") => Some(Self::Failed),
            _ => None,
        }
    }
    pub fn settle(self, next: Self) -> Option<Self> {
        (self == Self::Pending && next != Self::Pending).then_some(next)
    }
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::*;
    use wasm_bindgen::JsValue;
    use web_sys::{Document, Element, Event};

    fn root(document: &Document) -> Result<Element, JsValue> {
        document
            .document_element()
            .ok_or_else(|| JsValue::from_str("Browser document root is missing"))
    }
    fn phase(root: &Element) -> Result<Phase, JsValue> {
        Phase::from_attribute(root.get_attribute("data-zxs-startup").as_deref())
            .ok_or_else(|| JsValue::from_str("Invalid application startup state"))
    }
    pub fn begin(document: &Document) -> Result<(), JsValue> {
        let root = root(document)?;
        if phase(&root)? != Phase::Pending {
            return Err(JsValue::from_str("Application startup is already settled"));
        }
        root.set_attribute("data-zxs-startup", "pending")?;
        root.set_attribute("data-zxs-fonts", "pending")?;
        Ok(())
    }
    pub fn ensure_pending(document: &Document) -> Result<(), JsValue> {
        if phase(&root(document)?)? == Phase::Pending {
            Ok(())
        } else {
            Err(JsValue::from_str("Application startup is already settled"))
        }
    }
    pub fn ready(document: &Document) -> Result<(), JsValue> {
        let root = root(document)?;
        if phase(&root)?.settle(Phase::Ready).is_none() {
            return Err(JsValue::from_str("Application startup is already settled"));
        }
        root.set_attribute("data-zxs-runtime", "web-interface")?;
        root.set_attribute("data-zxs-ready", "true")?;
        root.set_attribute("data-zxs-startup", "ready")?;
        document.dispatch_event(&Event::new(READY_EVENT)?)?;
        // A synchronous listener may reject startup and publish its failure page.
        if phase(&root)? == Phase::Failed {
            return Err(JsValue::from_str("Application startup was rejected"));
        }
        Ok(())
    }
    pub fn failed(document: &Document, error: &JsValue) {
        let root = match root(document) {
            Ok(root) => root,
            Err(reporting_error) => {
                web_sys::console::error_3(
                    &"Application startup failed without a document root".into(),
                    error,
                    &reporting_error,
                );
                return;
            }
        };
        let current = match phase(&root) {
            Ok(current) => current,
            Err(reporting_error) => {
                web_sys::console::error_3(
                    &"Application startup state is invalid".into(),
                    error,
                    &reporting_error,
                );
                return;
            }
        };
        if current.settle(Phase::Failed).is_none() {
            return;
        }
        for result in [
            root.set_attribute("data-zxs-startup", "failed"),
            root.remove_attribute("data-zxs-runtime"),
            root.remove_attribute("data-zxs-ready"),
        ] {
            if let Err(error) = result {
                web_sys::console::warn_2(
                    &"Could not update failed startup attributes".into(),
                    &error,
                );
            }
        }
        web_sys::console::error_2(&JsValue::from_str("Application startup failed"), error);
        if let Err(error) =
            Event::new(FAILED_EVENT).and_then(|event| document.dispatch_event(&event))
        {
            web_sys::console::warn_2(&"Could not announce startup failure".into(), &error);
        }
    }
}
#[cfg(target_arch = "wasm32")]
pub(crate) use browser::*;

#[cfg(test)]
#[path = "../../../test/interface/unit/startup/tests.rs"]
mod tests;
