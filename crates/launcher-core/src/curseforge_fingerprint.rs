//! CurseForge 文件指纹。
//!
//! ## 这个哈希是什么
//!
//! CurseForge 用 MurmurHash2（32 位，种子 1）给每个上传的文件算指纹，并提供
//! `POST /v1/fingerprints/432` 接口：提交一批指纹，换回对应的工程与文件信息。
//!
//! 这是识别本地模组最可靠的方式。按文件名猜是行不通的——整合包里的 jar 常被
//! 改名（`jei-1.20.1-15.2.0.27.jar`、`JustEnoughItems.jar`、作者自己起的名），
//! 而指纹直接由字节内容决定，改过名的同一文件仍然匹配。
//!
//! ## 一个必须照做的细节
//!
//! CurseForge 计算时**先剔除空白字节**（`\t` 0x09、`\n` 0x0A、`\r` 0x0D、空格 0x20），
//! 再对剩余字节做哈希。这不是可选的优化：按原始字节算出的值永远对不上服务端，
//! 表现为「所有本地模组都识别不出来」，而两边代码看上去都「只是算了个 MurmurHash2」。
//!
//! 这个规则来自 PCL2 的实现（`LocalResourceFile.vb` 的 `CurseForgeHash`），也是
//! CurseForge 官方客户端的行为。

/// 计算一个文件的 CurseForge 指纹（MurmurHash2, seed 1）。
///
/// 返回 `None` 表示读取失败。不在这里做缓存：调用方通常一次处理一批文件，
/// 缓存策略（按修改时间还是按大小）属于它的判断。
pub fn file_fingerprint(path: &std::path::Path) -> Option<u32> {
    let bytes = std::fs::read(path).ok()?;
    Some(fingerprint_of(&bytes))
}

/// 对已读入内存的字节计算指纹。便于测试与「已经读过一次」的场景。
pub fn fingerprint_of(bytes: &[u8]) -> u32 {
    /*
     * 先剔除空白字节。
     *
     * 用 `filter` 而不是原地修改：调用方可能还要用原始字节（例如校验 SHA-1），
     * 就地删字节会让两份计算互相污染。
     */
    let data: Vec<u8> = bytes
        .iter()
        .copied()
        .filter(|b| !matches!(b, 0x09 | 0x0A | 0x0D | 0x20))
        .collect();
    murmur2_seed1(&data)
}

/// MurmurHash2 的 32 位实现，种子固定为 1（CurseForge 的要求）。
///
/// 按小端序取 4 字节一组，末尾 1–3 字节单独处理。移位与乘法都用 `wrapping_*`：
/// 这是无符号溢出，正常路径依赖它回绕，用普通乘法在 debug 构建下会 panic。
fn murmur2_seed1(data: &[u8]) -> u32 {
    const M: u32 = 0x5bd1_e995;
    let mut h: u32 = 1 ^ (data.len() as u32);
    let mut chunks = data.chunks_exact(4);
    for chunk in &mut chunks {
        let mut k = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        k = k.wrapping_mul(M);
        k ^= k >> 24;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M);
        h ^= k;
    }
    let tail = chunks.remainder();
    // 与 PCL2 的分支顺序一致：3 字节时先异或前两字节再异或第三字节。
    match tail.len() {
        3 => {
            h ^= u32::from(tail[0]) | (u32::from(tail[1]) << 8);
            h ^= u32::from(tail[2]) << 16;
            h = h.wrapping_mul(M);
        }
        2 => {
            h ^= u32::from(tail[0]) | (u32::from(tail[1]) << 8);
            h = h.wrapping_mul(M);
        }
        1 => {
            h ^= u32::from(tail[0]);
            h = h.wrapping_mul(M);
        }
        _ => {}
    }
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^= h >> 15;
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与 PCL2 的算法对齐：空白字节被剔除后结果不变。
    #[test]
    fn whitespace_bytes_do_not_affect_the_fingerprint() {
        /*
         * 两个输入的非空白字节必须完全相同：
         *   padded 去掉 0x20 / 0x0A / 0x09 / 0x0D 后是 "PK\x03\x04abcdefgh"
         *   这正是 clean 的内容。
         */
        let padded = b"PK\x03\x04 abc\ndef\tgh\r";
        let clean = b"PK\x03\x04abcdefgh";
        assert_eq!(fingerprint_of(padded), fingerprint_of(clean));
        // 反向确认：少一个字节就不相等，否则这条测试可能是恒真的。
        assert_ne!(fingerprint_of(clean), fingerprint_of(b"PK\x03\x04abcdefg"));
    }

    #[test]
    fn is_stable_across_calls() {
        let data = b"some mod bytes";
        assert_eq!(fingerprint_of(data), fingerprint_of(data));
    }

    #[test]
    fn different_content_gives_different_value() {
        assert_ne!(fingerprint_of(b"aaa"), fingerprint_of(b"aab"));
    }

    /// 空文件也要给出确定的值，而不是 panic。
    #[test]
    fn empty_input_is_defined() {
        assert_eq!(fingerprint_of(b""), murmur2_seed1(&[]));
    }

    /// 尾部长度 1/2/3 都走不同分支，逐一确认不 panic 且结果彼此不同。
    #[test]
    fn tail_branches_are_reachable() {
        let a = fingerprint_of(b"abcde"); // 4+1
        let b = fingerprint_of(b"abcdef"); // 4+2
        let c = fingerprint_of(b"abcdefg"); // 4+3
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }
}
