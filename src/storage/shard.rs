use std::path::PathBuf;

pub fn shard_path(base: &str, checksum: &str) -> PathBuf {
    let level1 = &checksum[0..2];
    let level2 = &checksum[2..4];
    PathBuf::from(base).join(level1).join(level2).join(checksum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_path_two_levels() {
        // SHA-256 of "hello" = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let checksum = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let path = shard_path("/data/blobs", checksum);
        assert_eq!(
            path,
            PathBuf::from("/data/blobs/2c/f2/2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
        );
    }

    #[test]
    fn test_shard_path_different_checksum() {
        let checksum = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab";
        let path = shard_path("/store", checksum);
        assert_eq!(
            path,
            PathBuf::from("/store/ab/cd/abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab")
        );
    }
}
