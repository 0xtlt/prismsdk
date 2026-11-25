//! What a store wants to know about an app's privacy, from `Prism.toml`.
//!
//! - `[permissions]`: the protected resources the app asks for, with the
//!   text the system shows (Info.plist usage descriptions, Android
//!   permissions, sandbox entitlements). On Apple platforms each one also
//!   turns on the prismsdk feature that compiles its code in: App Store
//!   Connect wants a purpose string for every protected API the binary
//!   references, used or not (ITMS-90683).
//! - `[privacy]`, `[privacy.apis]`, `[privacy.collected]`: the App Store
//!   privacy manifest (`PrivacyInfo.xcprivacy`, ITMS-91053). The bundlers
//!   add the required-reason APIs they find in each binary
//!   (`scripts/apple-privacy.py`).
//!
//! ```toml
//! [privacy]
//! tracking = false                         # NSPrivacyTracking
//! tracking_domains = ["ads.example.com"]   # needed when tracking = true
//! manifest = "PrivacyInfo.xcprivacy"       # merged in (the default when it exists)
//!
//! [privacy.apis]                           # required-reason APIs of the app's own code
//! user_defaults = ["CA92.1"]
//!
//! [privacy.collected]                      # data the app (or its server) collects
//! email_address = ["app_functionality", "linked"]
//! crash_data = ["analytics"]
//! ```

use std::path::{Path, PathBuf};

/// The `[permissions]` names `prism` knows, in the order the docs list them.
pub const PERMISSIONS: [&str; 19] = [
    "location",
    "location_always",
    "camera",
    "microphone",
    "photos",
    "motion",
    "face_id",
    "contacts",
    "calendars",
    "reminders",
    "bluetooth",
    "speech",
    "tracking",
    "health",
    "nfc",
    "media_library",
    "homekit",
    "siri",
    "local_network",
];

/// A `[permissions]` name, checked.
pub fn permission(name: &str) -> Result<String, String> {
    if PERMISSIONS.contains(&name) {
        Ok(name.to_string())
    } else {
        Err(format!(
            "Prism.toml [permissions] {name}: not a permission prism knows ({})",
            PERMISSIONS.join(", ")
        ))
    }
}

/// prismsdk's feature for a permission whose code is compiled in only when
/// the app declares it (Apple platforms).
pub fn permission_feature(name: &str) -> Option<&'static str> {
    Some(match name {
        "location" => "location",
        "location_always" => "location-always",
        "camera" => "camera",
        "microphone" => "microphone",
        "photos" => "photo-library",
        "face_id" => "face-id",
        _ => return None,
    })
}

/// A required-reason API category: `[privacy.apis]`'s name, Apple's, and
/// the reasons Apple approves for it.
const API_CATEGORIES: [(&str, &str, &[&str]); 5] = [
    (
        "file_timestamp",
        "NSPrivacyAccessedAPICategoryFileTimestamp",
        &["DDA9.1", "C617.1", "3B52.1", "0A2A.1"],
    ),
    (
        "system_boot_time",
        "NSPrivacyAccessedAPICategorySystemBootTime",
        &["35F9.1", "8FFB.1", "3D61.1"],
    ),
    (
        "disk_space",
        "NSPrivacyAccessedAPICategoryDiskSpace",
        &["85F4.1", "E174.1", "7D9E.1", "B728.1"],
    ),
    (
        "active_keyboards",
        "NSPrivacyAccessedAPICategoryActiveKeyboards",
        &["3EC4.1", "54BD.1"],
    ),
    (
        "user_defaults",
        "NSPrivacyAccessedAPICategoryUserDefaults",
        &["CA92.1", "1C8F.1", "C56D.1", "AC6B.1"],
    ),
];

/// `[privacy.collected]`'s names for Apple's collected data types
/// (`NSPrivacyCollectedDataType…`).
const DATA_TYPES: [(&str, &str); 35] = [
    ("name", "Name"),
    ("email_address", "EmailAddress"),
    ("phone_number", "PhoneNumber"),
    ("physical_address", "PhysicalAddress"),
    ("other_contact_info", "OtherUserContactInfo"),
    ("health", "Health"),
    ("fitness", "Fitness"),
    ("payment_info", "PaymentInfo"),
    ("credit_info", "CreditInfo"),
    ("other_financial_info", "OtherFinancialInfo"),
    ("precise_location", "PreciseLocation"),
    ("coarse_location", "CoarseLocation"),
    ("sensitive_info", "SensitiveInfo"),
    ("contacts", "Contacts"),
    ("emails_or_text_messages", "EmailsOrTextMessages"),
    // Apple's spelling.
    ("photos_or_videos", "PhotosorVideos"),
    ("audio_data", "AudioData"),
    ("gameplay_content", "GameplayContent"),
    ("customer_support", "CustomerSupport"),
    ("other_user_content", "OtherUserContent"),
    ("browsing_history", "BrowsingHistory"),
    ("search_history", "SearchHistory"),
    ("user_id", "UserID"),
    ("device_id", "DeviceID"),
    ("purchase_history", "PurchaseHistory"),
    ("product_interaction", "ProductInteraction"),
    ("advertising_data", "AdvertisingData"),
    ("other_usage_data", "OtherUsageData"),
    ("crash_data", "CrashData"),
    ("performance_data", "PerformanceData"),
    ("other_diagnostic_data", "OtherDiagnosticData"),
    ("environment_scanning", "EnvironmentScanning"),
    ("hands", "Hands"),
    ("head", "Head"),
    ("other_data_types", "OtherDataTypes"),
];

/// Why the data is collected (`NSPrivacyCollectedDataTypePurpose…`).
const PURPOSES: [(&str, &str); 6] = [
    ("third_party_advertising", "ThirdPartyAdvertising"),
    ("developer_advertising", "DeveloperAdvertising"),
    ("analytics", "Analytics"),
    ("product_personalization", "ProductPersonalization"),
    ("app_functionality", "AppFunctionality"),
    ("other", "Other"),
];

/// One `[privacy.collected]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collected {
    /// `NSPrivacyCollectedDataType…`.
    pub kind: String,
    /// Linked to the user's identity.
    pub linked: bool,
    /// Used to track the user.
    pub tracking: bool,
    /// `NSPrivacyCollectedDataTypePurpose…`.
    pub purposes: Vec<String>,
}

/// The app's part of its privacy manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Privacy {
    /// `NSPrivacyTracking`.
    pub tracking: bool,
    /// `NSPrivacyTrackingDomains`.
    pub tracking_domains: Vec<String>,
    /// Required-reason categories of the app's own code, with their
    /// reasons (`NSPrivacyAccessedAPIType…`).
    pub apis: Vec<(String, Vec<String>)>,
    /// `NSPrivacyCollectedDataTypes`.
    pub collected: Vec<Collected>,
    /// A `PrivacyInfo.xcprivacy` of the app's, merged in.
    pub manifest: Option<PathBuf>,
}

impl Privacy {
    /// Reads `[privacy]`, `[privacy.apis]` and `[privacy.collected]` from
    /// the parsed Prism.toml (`section.key` → value).
    pub fn load(
        config: &std::collections::HashMap<String, String>,
        root: &Path,
        has_tracking_text: bool,
    ) -> Result<Self, String> {
        let get = |k: &str| config.get(k).filter(|v| !v.is_empty()).cloned();
        let tracking = match get("privacy.tracking").as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(v) => {
                return Err(format!(
                    "Prism.toml [privacy] tracking: true or false, not {v:?}"
                ));
            }
        };
        let tracking_domains = get("privacy.tracking_domains")
            .map(|v| crate::config::parse_list(&v))
            .unwrap_or_default()
            .into_iter()
            .map(|d| domain(&d))
            .collect::<Result<Vec<_>, _>>()?;
        if tracking && tracking_domains.is_empty() {
            return Err("Prism.toml [privacy] tracking = true: list the domains that track in tracking_domains (Apple requires at least one)".into());
        }
        if tracking && !has_tracking_text {
            return Err("Prism.toml [privacy] tracking = true: add `tracking = \"why\"` under [permissions] (NSUserTrackingUsageDescription, the App Tracking Transparency prompt)".into());
        }
        let mut keys: Vec<&String> = config.keys().collect();
        keys.sort();
        let mut apis = Vec::new();
        let mut collected = Vec::new();
        for key in keys {
            let value = &config[key];
            if let Some(name) = key.strip_prefix("privacy.apis.") {
                let (_, apple, approved) = API_CATEGORIES
                    .iter()
                    .find(|(n, ..)| *n == name)
                    .ok_or_else(|| {
                        format!(
                            "Prism.toml [privacy.apis] {name}: not a required-reason API category ({})",
                            API_CATEGORIES.map(|c| c.0).join(", ")
                        )
                    })?;
                let reasons = crate::config::parse_list(value);
                if reasons.is_empty() {
                    return Err(format!(
                        "Prism.toml [privacy.apis] {name}: give at least one reason ({})",
                        approved.join(", ")
                    ));
                }
                if let Some(bad) = reasons.iter().find(|r| !approved.contains(&r.as_str())) {
                    return Err(format!(
                        "Prism.toml [privacy.apis] {name}: {bad:?} is not one of Apple's reasons for it ({}); a newer reason can go in PrivacyInfo.xcprivacy ([privacy] manifest)",
                        approved.join(", ")
                    ));
                }
                apis.push((apple.to_string(), reasons));
            } else if let Some(name) = key.strip_prefix("privacy.collected.") {
                collected.push(collected_entry(name, value)?);
            } else if let Some(name) = key.strip_prefix("privacy.")
                && !matches!(name, "tracking" | "tracking_domains" | "manifest")
            {
                return Err(format!(
                    "Prism.toml [privacy] {name}: unknown (tracking, tracking_domains, manifest; [privacy.apis], [privacy.collected])"
                ));
            }
        }
        let manifest = match get("privacy.manifest") {
            Some(m) => {
                let path = crate::resolve(root, &m);
                if !path.is_file() {
                    return Err(format!(
                        "Prism.toml [privacy] manifest: {} does not exist",
                        path.display()
                    ));
                }
                Some(path)
            }
            None => Some(root.join("PrivacyInfo.xcprivacy")).filter(|p| p.is_file()),
        };
        Ok(Privacy {
            tracking,
            tracking_domains,
            apis,
            collected,
            manifest,
        })
    }

    /// What `scripts/apple-privacy.py` reads (`PRISM_PRIVACY`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "tracking": self.tracking,
            "tracking_domains": self.tracking_domains,
            "apis": self.apis.iter().map(|(c, r)| serde_json::json!({"category": c, "reasons": r})).collect::<Vec<_>>(),
            "collected": self.collected.iter().map(|c| serde_json::json!({
                "type": c.kind,
                "linked": c.linked,
                "tracking": c.tracking,
                "purposes": c.purposes,
            })).collect::<Vec<_>>(),
            "manifest": self.manifest.as_ref().map(|m| m.display().to_string()),
        })
    }
}

/// `email_address = ["app_functionality", "linked"]`: a data type, its
/// purposes, and the flags `linked` and `tracking`.
fn collected_entry(name: &str, value: &str) -> Result<Collected, String> {
    let (_, suffix) = DATA_TYPES.iter().find(|(n, _)| *n == name).ok_or_else(|| {
        format!(
            "Prism.toml [privacy.collected] {name}: not one of Apple's data types ({})",
            DATA_TYPES.map(|d| d.0).join(", ")
        )
    })?;
    let mut entry = Collected {
        kind: format!("NSPrivacyCollectedDataType{suffix}"),
        linked: false,
        tracking: false,
        purposes: Vec::new(),
    };
    for word in crate::config::parse_list(value) {
        match word.as_str() {
            "linked" => entry.linked = true,
            "tracking" => entry.tracking = true,
            w => {
                let (_, p) = PURPOSES.iter().find(|(n, _)| *n == w).ok_or_else(|| {
                    format!(
                        "Prism.toml [privacy.collected] {name}: {w:?} is neither a purpose ({}) nor linked / tracking",
                        PURPOSES.map(|p| p.0).join(", ")
                    )
                })?;
                entry
                    .purposes
                    .push(format!("NSPrivacyCollectedDataTypePurpose{p}"));
            }
        }
    }
    if entry.purposes.is_empty() {
        return Err(format!(
            "Prism.toml [privacy.collected] {name}: say why it is collected ({})",
            PURPOSES.map(|p| p.0).join(", ")
        ));
    }
    Ok(entry)
}

/// A tracking domain: a host name, lower case.
fn domain(raw: &str) -> Result<String, String> {
    let host = raw.trim().to_ascii_lowercase();
    let ok = host.contains('.')
        && !host.starts_with(['.', '-'])
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if ok {
        Ok(host)
    } else {
        Err(format!(
            "Prism.toml [privacy] tracking_domains: {raw:?} is not a domain (\"ads.example.com\")"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn load(toml: &str, tracking_text: bool) -> Result<Privacy, String> {
        let config: HashMap<String, String> = crate::config::parse(toml);
        Privacy::load(&config, Path::new("/nonexistent"), tracking_text)
    }

    #[test]
    fn reads_the_privacy_sections() {
        let p = load(
            "[privacy]\ntracking = true\ntracking_domains = [\"Ads.Example.com\"]\n\
             [privacy.apis]\nuser_defaults = [\"CA92.1\", \"1C8F.1\"]\n\
             [privacy.collected]\nemail_address = [\"app_functionality\", \"linked\"]\n\
             crash_data = [\"analytics\", \"tracking\"]\n",
            true,
        )
        .unwrap();
        assert!(p.tracking);
        assert_eq!(p.tracking_domains, ["ads.example.com"]);
        assert_eq!(
            p.apis,
            [(
                "NSPrivacyAccessedAPICategoryUserDefaults".to_string(),
                vec!["CA92.1".to_string(), "1C8F.1".to_string()]
            )]
        );
        assert_eq!(
            p.collected,
            [
                Collected {
                    kind: "NSPrivacyCollectedDataTypeCrashData".into(),
                    linked: false,
                    tracking: true,
                    purposes: vec!["NSPrivacyCollectedDataTypePurposeAnalytics".into()],
                },
                Collected {
                    kind: "NSPrivacyCollectedDataTypeEmailAddress".into(),
                    linked: true,
                    tracking: false,
                    purposes: vec!["NSPrivacyCollectedDataTypePurposeAppFunctionality".into()],
                },
            ]
        );
        let json = p.to_json();
        assert_eq!(json["tracking_domains"][0], "ads.example.com");
        assert_eq!(json["apis"][0]["reasons"][1], "1C8F.1");
        assert_eq!(json["collected"][1]["linked"], true);
        assert!(json["manifest"].is_null());
    }

    #[test]
    fn nothing_declared_is_no_tracking() {
        assert_eq!(
            load("[app]\nname = \"x\"\n", false).unwrap(),
            Privacy::default()
        );
    }

    #[test]
    fn refuses_what_apple_would() {
        for (toml, says) in [
            ("[privacy]\ntracking = yes\n", "true or false"),
            ("[privacy]\ntracking = true\n", "tracking_domains"),
            (
                "[privacy]\ntracking = true\ntracking_domains = [\"a.com\"]\n",
                "NSUserTrackingUsageDescription",
            ),
            (
                "[privacy]\ntracking_domains = [\"https://a.com\"]\n",
                "not a domain",
            ),
            (
                "[privacy.apis]\nuser_defaults = [\"CA92.2\"]\n",
                "not one of Apple's reasons",
            ),
            (
                "[privacy.apis]\nfile_timestamp = [\"CA92.1\"]\n",
                "not one of Apple's reasons",
            ),
            (
                "[privacy.apis]\nclipboard = [\"CA92.1\"]\n",
                "not a required-reason API category",
            ),
            ("[privacy.apis]\ndisk_space = []\n", "at least one reason"),
            (
                "[privacy.collected]\nemail = [\"analytics\"]\n",
                "not one of Apple's data types",
            ),
            (
                "[privacy.collected]\nemail_address = [\"linked\"]\n",
                "say why",
            ),
            (
                "[privacy.collected]\nemail_address = [\"marketing\"]\n",
                "neither a purpose",
            ),
            ("[privacy]\ntracker = true\n", "unknown"),
            (
                "[privacy]\nmanifest = \"nope.xcprivacy\"\n",
                "does not exist",
            ),
        ] {
            let err = load(toml, false).unwrap_err();
            assert!(err.contains(says), "{toml:?}: {err}");
        }
    }

    #[test]
    fn permissions_are_known_names_and_some_turn_on_features() {
        assert_eq!(permission("camera"), Ok("camera".into()));
        assert!(
            permission("photo")
                .unwrap_err()
                .contains("location, location_always")
        );
        assert_eq!(permission_feature("photos"), Some("photo-library"));
        assert_eq!(
            permission_feature("location_always"),
            Some("location-always")
        );
        assert_eq!(permission_feature("face_id"), Some("face-id"));
        assert_eq!(permission_feature("motion"), None);
        assert_eq!(permission_feature("contacts"), None);
        for p in PERMISSIONS {
            assert!(permission(p).is_ok());
        }
    }
}
