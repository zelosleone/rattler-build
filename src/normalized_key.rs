use rattler_conda_types::PackageName;
use serde::{Deserialize, Serialize};
use std::hash::Hash;

/// A key in a variant configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct NormalizedKey(pub String);

impl NormalizedKey {
    /// Returns the normalized form of the key.
    pub fn normalize(&self) -> String {
        self.0
            .chars()
            .map(|c| match c {
                '-' | '_' | '.' => '_',
                x => x,
            })
            .collect()
    }
}

impl Serialize for NormalizedKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.normalize().serialize(serializer)
    }
}

impl Hash for NormalizedKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.normalize().hash(state)
    }
}

impl PartialEq for NormalizedKey {
    fn eq(&self, other: &Self) -> bool {
        self.normalize() == other.normalize()
    }
}

impl Eq for NormalizedKey {}

impl PartialOrd for NormalizedKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NormalizedKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.normalize().cmp(&other.normalize())
    }
}

// For convenience, implement From<String> and From<&str>
impl From<String> for NormalizedKey {
    fn from(s: String) -> Self {
        NormalizedKey(s)
    }
}

impl From<&str> for NormalizedKey {
    fn from(s: &str) -> Self {
        NormalizedKey(s.to_string())
    }
}

impl From<&PackageName> for NormalizedKey {
    fn from(p: &PackageName) -> Self {
        p.as_normalized().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize() {
        // Test normalization of different characters
        let key = NormalizedKey("test-key".to_string());
        assert_eq!(key.normalize(), "test_key");

        let key = NormalizedKey("test_key".to_string());
        assert_eq!(key.normalize(), "test_key");

        let key = NormalizedKey("test.key".to_string());
        assert_eq!(key.normalize(), "test_key");

        let key = NormalizedKey("test-key_with.mixed".to_string());
        assert_eq!(key.normalize(), "test_key_with_mixed");

        // Test that other characters are preserved
        let key = NormalizedKey("TestKey123".to_string());
        assert_eq!(key.normalize(), "TestKey123");

        let key = NormalizedKey("test@key#123".to_string());
        assert_eq!(key.normalize(), "test@key#123");
    }

    #[test]
    fn test_equality() {
        let key1 = NormalizedKey("test-key".to_string());
        let key2 = NormalizedKey("test_key".to_string());
        let key3 = NormalizedKey("test.key".to_string());
        let key4 = NormalizedKey("test@key".to_string());

        // Keys with different separators should be equal
        assert_eq!(key1, key2);
        assert_eq!(key1, key3);
        assert_eq!(key2, key3);

        // Keys with different content should not be equal
        assert_ne!(key1, key4);
    }

    #[test]
    fn test_hash() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(NormalizedKey("test-key".to_string()));

        // Should find the key even with different separators
        assert!(set.contains(&NormalizedKey("test_key".to_string())));
        assert!(set.contains(&NormalizedKey("test.key".to_string())));

        // Should not find a different key
        assert!(!set.contains(&NormalizedKey("test@key".to_string())));
    }

    #[test]
    fn test_ordering() {
        let key1 = NormalizedKey("aaa-bbb".to_string());
        let key2 = NormalizedKey("aaa_ccc".to_string());
        let key3 = NormalizedKey("bbb.aaa".to_string());

        assert!(key1 < key2);
        assert!(key2 < key3);
        assert!(key1 < key3);

        // Test that normalized forms are compared
        let key4 = NormalizedKey("aaa-bbb".to_string());
        let key5 = NormalizedKey("aaa_bbb".to_string());
        assert_eq!(key4.cmp(&key5), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_from_implementations() {
        // Test From<String>
        let key1: NormalizedKey = "test-key".to_string().into();
        assert_eq!(key1.0, "test-key");

        // Test From<&str>
        let key2: NormalizedKey = "test-key".into();
        assert_eq!(key2.0, "test-key");

        // Test equality of different From implementations
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_serialization() {
        let key = NormalizedKey("test-key".to_string());
        let serialized = serde_json::to_string(&key).unwrap();

        // Should serialize to the normalized form
        assert_eq!(serialized, "\"test_key\"");
    }

    #[test]
    fn test_deserialization() {
        // Can deserialize from non-normalized form
        let key: NormalizedKey = serde_json::from_str("\"test-key\"").unwrap();
        assert_eq!(key.0, "test-key");
        assert_eq!(key.normalize(), "test_key");

        // Can deserialize from normalized form
        let key: NormalizedKey = serde_json::from_str("\"test_key\"").unwrap();
        assert_eq!(key.0, "test_key");
        assert_eq!(key.normalize(), "test_key");
    }

    #[test]
    fn test_from_package_name() {
        let pkg_name: PackageName = "test-package".parse().unwrap();
        let key: NormalizedKey = (&pkg_name).into();

        // PackageName normalization uses hyphens, but NormalizedKey converts to underscores
        assert_eq!(key.normalize(), "test_package");
    }

    #[test]
    fn test_edge_cases() {
        // Empty string
        let key = NormalizedKey("".to_string());
        assert_eq!(key.normalize(), "");

        // Only separators
        let key = NormalizedKey("-_.".to_string());
        assert_eq!(key.normalize(), "___");

        // Multiple consecutive separators
        let key = NormalizedKey("test--key__with..dots".to_string());
        assert_eq!(key.normalize(), "test__key__with__dots");

        // Unicode characters
        let key = NormalizedKey("tëst-këy".to_string());
        assert_eq!(key.normalize(), "tëst_këy");
    }
}
