//! LiteLoader's published index and timestamped Maven snapshots.
use super::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const INDEX: &str = "https://dl.liteloader.com/versions/versions.json";
const CLEANROOM: &str = "https://repo.cleanroommc.com/releases/com/cleanroommc/cleanroom/";

pub(super) fn client(
    network: &download_engine::DownloadSettings,
) -> Result<reqwest::Client, String> {
    network
        .configure_metadata_client(
            reqwest::Client::builder()
                .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
                .https_only(true)
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() < 3 && allowed_url(attempt.url()).is_ok() {
                        attempt.follow()
                    } else {
                        attempt.stop()
                    }
                })),
            &[
                "dl.liteloader.com",
                "repo.mumfrey.com",
                "repo.spongepowered.org",
                "repo.cleanroommc.com",
                "maven.cleanroommc.com",
                "nexus.jsdu.cn",
                "maven.legacyfabric.net",
                "repo.legacyfabric.net",
                "meta.legacyfabric.net",
                "maven.fabricmc.net",
                "libraries.minecraft.net",
                "repo1.maven.org",
                "repo.maven.apache.org",
            ],
        )
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())
}

pub(super) fn allowed_url(url: &reqwest::Url) -> Result<(), String> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(
            url.host_str(),
            Some(
                "dl.liteloader.com"
                    | "repo.mumfrey.com"
                    | "repo.spongepowered.org"
                    | "repo.cleanroommc.com"
                    | "maven.cleanroommc.com"
                    | "nexus.jsdu.cn"
                    | "maven.legacyfabric.net"
                    | "repo.legacyfabric.net"
                    | "meta.legacyfabric.net"
                    | "maven.fabricmc.net"
                    | "libraries.minecraft.net"
                    | "repo1.maven.org"
                    | "repo.maven.apache.org"
            )
        )
    {
        return Err("加载器依赖地址不在获准的仓库中。".into());
    }
    Ok(())
}

async fn get(client: &reqwest::Client, url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let url = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    allowed_url(&url)?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    super::bounded_response(response, limit).await
}

pub(crate) async fn checksum(
    url: &str,
    network: &download_engine::DownloadSettings,
) -> Result<String, String> {
    let bytes = get(&client(network)?, &format!("{url}.sha1"), 4096).await?;
    let hash = std::str::from_utf8(&bytes)
        .map_err(|e| e.to_string())?
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_owned();
    if hash.len() != 40 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("官方 SHA-1 校验值无效。".into());
    }
    Ok(hash)
}

#[derive(Clone)]
struct Release {
    version: String,
    url: String,
    stable: bool,
    libraries: Vec<Value>,
}

fn releases(index: &Value, game: &str) -> Result<Vec<Release>, String> {
    let Some(branch) = index.get("versions").and_then(|v| v.get(game)) else {
        return Ok(vec![]);
    };
    let mut out = BTreeMap::new();
    if let Some(entries) = branch
        .get("artefacts")
        .and_then(|v| v.get("com.mumfrey:liteloader"))
        .and_then(Value::as_object)
    {
        let repo = branch
            .get("repo")
            .and_then(|v| v.get("url"))
            .and_then(Value::as_str)
            .ok_or("LiteLoader 缺少仓库。")?;
        let repo = repo.replacen("http://", "https://", 1);
        allowed_url(&reqwest::Url::parse(&repo).map_err(|e| e.to_string())?)?;
        for (key, release) in entries {
            if key == "latest" {
                continue;
            }
            let version = release
                .get("version")
                .and_then(Value::as_str)
                .ok_or("LiteLoader 版本无效。")?;
            safe_version(version)?;
            let file = release
                .get("file")
                .and_then(Value::as_str)
                .ok_or("LiteLoader 文件名无效。")?;
            safe_version(file)?;
            let url = format!("{repo}com/mumfrey/liteloader/{game}/{file}");
            let libraries = release
                .get("libraries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            out.insert(
                version.to_owned(),
                Release {
                    version: version.into(),
                    url,
                    stable: true,
                    libraries,
                },
            );
        }
    }
    Ok(out.into_values().collect())
}

fn xml_value<'a>(text: &'a str, tag: &str) -> Result<&'a str, String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let value = text
        .split_once(&open)
        .and_then(|(_, s)| s.split_once(&close))
        .map(|(s, _)| s.trim())
        .ok_or("Maven 版本元数据无效。")?;
    safe_version(value)?;
    Ok(value)
}

async fn lite_releases(game: &str, client: &reqwest::Client) -> Result<Vec<Release>, String> {
    let index: Value = serde_json::from_slice(&get(client, INDEX, 4 * 1024 * 1024).await?)
        .map_err(|e| e.to_string())?;
    let mut out = releases(&index, game)?;
    if let Some(snapshot) = index
        .get("versions")
        .and_then(|v| v.get(game))
        .and_then(|v| v.get("snapshots"))
        .and_then(|v| v.get("com.mumfrey:liteloader"))
        .and_then(|v| v.get("latest"))
    {
        let xml = get(client, &format!("https://repo.mumfrey.com/content/repositories/snapshots/com/mumfrey/liteloader/{game}-SNAPSHOT/maven-metadata.xml"), 1024 * 1024).await;
        match xml {
            Ok(bytes) => {
                let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
                let timestamp = xml_value(text, "timestamp")?;
                let build = xml_value(text, "buildNumber")?;
                out.push(Release {
                    version: format!("{timestamp}-{build}"), stable: false,
                    url: format!("https://repo.mumfrey.com/content/repositories/snapshots/com/mumfrey/liteloader/{game}-SNAPSHOT/liteloader-{game}-{timestamp}-{build}-release.jar"),
                    libraries: snapshot.get("libraries").and_then(Value::as_array).cloned().unwrap_or_default(),
                });
            }
            Err(error) if out.is_empty() => return Err(error),
            Err(_) => {}
        }
    }
    out.sort_by(|a, b| super::compare_versions(&b.version, &a.version));
    Ok(out)
}

pub(super) async fn list_versions(
    kind: LoaderKind,
    game: &str,
    network: &download_engine::DownloadSettings,
) -> Result<Vec<LoaderVersion>, String> {
    let client = client(network)?;
    match kind {
        LoaderKind::LiteLoader => Ok(lite_releases(game, &client)
            .await?
            .into_iter()
            .map(|r| LoaderVersion {
                version: r.version,
                stable: r.stable,
            })
            .collect()),
        LoaderKind::Cleanroom => {
            if game != "1.12.2" {
                return Ok(vec![]);
            }
            let bytes = get(
                &client,
                &format!("{CLEANROOM}maven-metadata.xml"),
                1024 * 1024,
            )
            .await?;
            let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
            Ok(cleanroom_versions(text))
        }
        _ => Err("无效的旧加载器类型。".into()),
    }
}

fn cleanroom_versions(text: &str) -> Vec<LoaderVersion> {
    let mut versions: Vec<_> = text
        .split("<version>")
        .skip(1)
        .filter_map(|s| s.split_once("</version>").map(|(v, _)| v.trim()))
        .filter(|v| safe_version(v).is_ok())
        .map(|v| LoaderVersion {
            version: v.into(),
            stable: !v.contains("alpha") && !v.contains("beta"),
        })
        .collect();
    versions.sort_by(|a, b| super::compare_versions(&b.version, &a.version));
    versions.dedup_by(|a, b| a.version == b.version);
    versions
}

pub(super) async fn lite_profile(
    game: &str,
    version: &str,
    client: &reqwest::Client,
) -> Result<Value, String> {
    let release = lite_releases(game, client)
        .await?
        .into_iter()
        .find(|r| r.version == version || (!r.stable && version == format!("{game}-SNAPSHOT")))
        .ok_or("所选 LiteLoader 版本不存在。")?;
    let mut libraries = release.libraries;
    libraries.push(json!({
        "name": format!("com.mumfrey:liteloader:{version}"),
        "url": "https://dl.liteloader.com/repo/",
        "mfmclDownloadUrl": release.url,
    }));
    Ok(
        json!({ "inheritsFrom": game, "mainClass": "net.minecraft.launchwrapper.Launch",
        "arguments": { "game": ["--tweakClass", "com.mumfrey.liteloader.launch.LiteLoaderTweaker"] },
        "libraries": libraries }),
    )
}

pub(super) async fn native_library(
    root: &Path,
    value: &Value,
    engine: &DownloadEngine,
    client: &reqwest::Client,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<Library, String> {
    let mut library: Library = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    super::safe_maven_path(&library.name)?;
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    };
    if let Some(classifier) = library.natives.get(os) {
        let classifier = classifier.replace(
            "${arch}",
            if cfg!(target_pointer_width = "64") {
                "64"
            } else {
                "32"
            },
        );
        safe_version(&classifier)?;
        let relative = super::safe_maven_path(&format!("{}:{classifier}", library.name))?;
        let repository = value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("https://libraries.minecraft.net/");
        let repository = reqwest::Url::parse(repository).map_err(|e| e.to_string())?;
        allowed_url(&repository)?;
        let url = repository
            .join(&relative)
            .map_err(|e| e.to_string())?
            .to_string();
        let hash = get(client, &format!("{url}.sha1"), 4096).await?;
        let sha1 = std::str::from_utf8(&hash)
            .map_err(|e| e.to_string())?
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_owned();
        if sha1.len() != 40 || !sha1.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Native 校验值无效。".into());
        }
        engine
            .download(
                &ArtifactSpec {
                    id: format!("loader-native:{}:{classifier}", library.name),
                    label: library.name.clone(),
                    url: url.clone(),
                    mirror_url: None,
                    destination: root.join("libraries").join(&relative),
                    size: None,
                    sha1: Some(sha1.clone()),
                    sha256: None,
                },
                progress,
            )
            .await
            .map_err(|e| e.to_string())?;
        library.downloads.classifiers.insert(
            classifier,
            Artifact {
                path: Some(relative),
                url,
                size: None,
                sha1: Some(sha1),
            },
        );
    }
    Ok(library)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_index_uses_published_files_and_never_latest_aliases() {
        let value = json!({"versions": {"1.10.2": {"repo": {"url": "http://dl.liteloader.com/repo/"}, "artefacts": {"com.mumfrey:liteloader": {
            "hash": {"version":"1.10.2", "file":"liteloader-1.10.2.jar", "libraries":[{"name":"net.minecraft:launchwrapper:1.12"}]},
            "latest": {"version":"wrong"}
        }}}}});
        let found = releases(&value, "1.10.2").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].url,
            "https://dl.liteloader.com/repo/com/mumfrey/liteloader/1.10.2/liteloader-1.10.2.jar"
        );
        assert!(releases(&value, "1.21").unwrap().is_empty());
    }
    #[test]
    fn legacy_repository_boundaries_and_snapshot_fields_are_strict() {
        for url in [
            "http://repo.mumfrey.com/a",
            "https://repo.mumfrey.com.evil/a",
            "https://user@dl.liteloader.com/a",
            "https://127.0.0.1/a",
        ] {
            assert!(allowed_url(&reqwest::Url::parse(url).unwrap()).is_err());
        }
        assert_eq!(xml_value("<snapshot><timestamp>20171129.142212</timestamp><buildNumber>1</buildNumber></snapshot>", "timestamp").unwrap(), "20171129.142212");
        assert!(xml_value("<timestamp>../bad</timestamp>", "timestamp").is_err());
        let versions = cleanroom_versions(
            "<versions><version>0.7.14-alpha</version><version>0.4.0</version><version>../bad</version></versions>",
        );
        assert_eq!(versions.len(), 2);
        assert!(!versions[0].stable);
        assert!(versions[1].stable);
    }
}
