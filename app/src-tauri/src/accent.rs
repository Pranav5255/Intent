//! GNOME accent colour via the XDG desktop portal
//! (`org.freedesktop.appearance` / `accent-color`, an `(ddd)` sRGB triple).

/// `#rrggbb`, or `None` when the portal has no accent (or we're not on Linux).
pub async fn accent_hex() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        portal::read().await
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Keep the portal code type-checked on every Unix without calling it.
        #[cfg(unix)]
        let _ = portal::read;
        None
    }
}

pub fn to_hex(r: f64, g: f64, b: f64) -> Option<String> {
    let channel = |v: f64| (0.0..=1.0).contains(&v).then(|| (v * 255.0).round() as u8);
    Some(format!("#{:02x}{:02x}{:02x}", channel(r)?, channel(g)?, channel(b)?))
}

#[cfg(unix)]
mod portal {
    use zbus::zvariant::{OwnedValue, Value};

    const DEST: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const IFACE: &str = "org.freedesktop.portal.Settings";

    pub async fn read() -> Option<String> {
        let conn = zbus::Connection::session().await.ok()?;
        // ReadOne (portal v2); fall back to the deprecated Read, which double-wraps.
        for method in ["ReadOne", "Read"] {
            let reply = conn
                .call_method(Some(DEST), PATH, Some(IFACE), method, &("org.freedesktop.appearance", "accent-color"))
                .await;
            if let Ok(msg) = reply
                && let Ok(value) = msg.body().deserialize::<OwnedValue>()
                && let Some(hex) = rgb(&value)
            {
                return Some(hex);
            }
        }
        None
    }

    fn rgb(value: &Value<'_>) -> Option<String> {
        match value {
            Value::Value(inner) => rgb(inner),
            Value::Structure(s) => {
                let f: Vec<f64> = s
                    .fields()
                    .iter()
                    .filter_map(|v| match v {
                        Value::F64(x) => Some(*x),
                        Value::Value(inner) => match inner.as_ref() {
                            Value::F64(x) => Some(*x),
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect();
                match f.as_slice() {
                    [r, g, b] => super::to_hex(*r, *g, *b),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hex_from_triple() {
        assert_eq!(super::to_hex(0.207, 0.517, 0.894).as_deref(), Some("#3584e4"));
        assert_eq!(super::to_hex(-1.0, 0.0, 0.0), None);
    }
}
