// 构建脚本。
//
// ## 为什么在这里读 `.env`
//
// CurseForge 的默认 key 通过 `option_env!("MFMCL_CURSEFORGE_API_KEY")` 在**编译期**
// 注入。之前这条值只在你手动 `source scripts/tauri-env.sh` 之后才存在——忘了这一步，
// 构建出来的二进制就没有 key，CurseForge 页会要求用户自己填。这个失败是静默的：
// 构建成功、界面正常，只是功能少了一块，而没人会去检查一个「本来能开」的功能。
//
// 现在构建脚本自己从仓库根目录的 `.env` 读一次。优先级保持「真实环境变量优先」，
// 这样 CI 仍然可以用 `env:` 覆盖，本地也能只改 `.env`。
//
// ## 只读 `.env`，不执行它
//
// 与 `scripts/tauri-env.sh` 同一个理由：`.env` 是可以被随便写的东西，`source` 会让
// 其中任意一行 shell 代码在构建时执行。这里只做 `KEY=value` 的文本解析，忽略注释、
// 空行与引号包裹，不做变量展开。
//
// ## 为什么不能用 `std::env::set_var`
//
// 构建脚本里设置的变量只对构建脚本自身与本进程可见，`option_env!` 是在**编译
// crate 时**由 rustc 求值的，拿不到这里 set 的值。所以必须用 `cargo:rustc-env`：
// 它把变量交给 cargo，由 cargo 传给 rustc。

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=MFMCL_MICROSOFT_CLIENT_ID");
    /*
     * CurseForge 默认 key 通过 `option_env!` 编译期注入。不声明这一行的话，
     * 只改 `.env` / 环境变量而不动源码时 Cargo 不会重新编译，二进制里会一直
     * 保留上一次构建的 key（或一直是空），排查起来非常费解。
     */
    println!("cargo:rerun-if-env-changed=MFMCL_CURSEFORGE_API_KEY");

    // 构建脚本的工作目录是 `src-tauri/`，仓库根在上一级。
    let env_file = Path::new("..").join(".env");
    println!("cargo:rerun-if-changed={}", env_file.display());

    /*
     * 真实环境变量优先。
     *
     * CI 通过 workflow 的 `env:` 注入时不该被本地 `.env` 覆盖——否则一份残留的
     * 本地文件就能改变发布产物的内容。
     */
    let from_env = std::env::var("MFMCL_CURSEFORGE_API_KEY").ok();

    if let Some(key) = from_env.or_else(|| read_env_file(&env_file, "MFMCL_CURSEFORGE_API_KEY")) {
        let key = key.trim().to_owned();
        if !key.is_empty() {
            println!("cargo:rustc-env=MFMCL_CURSEFORGE_API_KEY={key}");
        }
    }

    tauri_build::build()
}

/// 从 `.env` 风格的文本里取出一个键的值。
///
/// 行为与 `scripts/tauri-env.sh` 保持一致：只认 `KEY=value`，去掉成对的单/双引号，
/// 跳过注释与空行，不做转义与变量展开。两处规则若分叉，同一个 `.env` 会在
/// 「shell 里能读到、构建时读不到」之间摇摆，这比两边都不支持更难排查。
fn read_env_file(path: &Path, wanted: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != wanted {
            continue;
        }
        return Some(unquote(value.trim()));
    }
    None
}

/// 去掉一层成对的引号；不成对或没有引号时原样返回。
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
            return value[1..value.len() - 1].to_owned();
        }
    }
    value.to_owned()
}
