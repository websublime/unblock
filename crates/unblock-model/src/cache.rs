//! Shared cache-key contract type (CF-11, spine §1.9).

/// Ready/blocked projection cache-key contract (CF-11).
///
/// Defined in `unblock-model` because **both** `unblock-policy` and `unblock-storage` need it but
/// neither may depend on the other. It is an opaque newtype over a `String` key; the key
/// construction (filter fingerprint, etc.) lives in `unblock-policy`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey(pub String);

#[cfg(test)]
mod tests {
    use super::CacheKey;
    use std::collections::HashMap;

    #[test]
    fn usable_as_hashmap_key() {
        let mut map = HashMap::new();
        map.insert(CacheKey("ready:open".to_string()), 1);
        assert_eq!(map.get(&CacheKey("ready:open".to_string())), Some(&1));
        assert_eq!(map.get(&CacheKey("other".to_string())), None);
    }
}
