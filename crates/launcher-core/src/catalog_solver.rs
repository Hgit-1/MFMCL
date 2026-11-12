use super::*;
use serde_json::Value;
use std::time::Duration;

const MAX_REQUESTS: usize = 128;
const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_CANDIDATES: usize = 64;
const MAX_NODES: usize = 4096;
const MAX_STATES: usize = 4096;
const MAX_PROJECTS: usize = 128;
const MAX_EDGES: usize = 4096;
const MAX_FRONTIER_EDGES: usize = 65_536;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectVersionLock {
    pub project_id: String,
    pub version_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DependencyOptions {
    /// Locks apply when the project is needed; unrelated locks do not install it.
    #[serde(default)]
    pub locks: Vec<ProjectVersionLock>,
}

#[derive(Clone)]
struct Node {
    project: String,
    version: CatalogVersion,
    dependencies: Vec<DependencyRef>,
    compatible: bool,
}

struct Repository<'a> {
    client: &'a reqwest::Client,
    base: &'a str,
    instance: &'a InstanceRecord,
    loader: &'a str,
    requests: usize,
    bytes: usize,
    nodes: BTreeMap<String, Node>,
    lists: BTreeMap<String, Vec<String>>,
    narrowed: bool,
}

impl Repository<'_> {
    async fn fetch(&mut self, url: reqwest::Url) -> Result<Value, String> {
        if self.requests >= MAX_REQUESTS || self.bytes >= MAX_TOTAL_BYTES {
            return Err("依赖求解达到元数据请求/总字节限额".into());
        }
        self.requests += 1;
        let limit = MAX_RESPONSE.min(MAX_TOTAL_BYTES - self.bytes);
        let bytes = tokio::time::timeout(Duration::from_secs(30), async {
            let response = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|_| "无法读取模组依赖")?
                .error_for_status()
                .map_err(|_| "模组依赖不可用")?;
            bounded(response, limit).await
        })
        .await
        .map_err(|_| "模组依赖请求超时（含响应正文）")??;
        self.bytes += bytes.len();
        serde_json::from_slice(&bytes).map_err(|_| "模组依赖响应无效".into())
    }
    fn insert(&mut self, value: &Value, expected: Option<&str>) -> Result<String, String> {
        let project = value
            .get("project_id")
            .and_then(Value::as_str)
            .ok_or("依赖缺少项目标识")?;
        identifier(project)?;
        if expected.is_some_and(|id| id != project) {
            return Err("依赖项目与版本不一致".into());
        }
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or("依赖缺少版本 ID")?;
        identifier(id)?;
        if let Some(previous) = self.nodes.get(id) {
            if previous.project != project {
                return Err("目录返回的版本 ID 对应多个项目".into());
            }
            return Ok(id.into());
        }
        if self.nodes.len() >= MAX_NODES {
            return Err("依赖求解达到缓存版本数量限额".into());
        }
        let dependencies: Vec<DependencyRef> = serde_json::from_value(
            value
                .get("dependencies")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(|_| "模组依赖字段无效")?;
        if dependencies.len() > 512 {
            return Err("单版本依赖条目超限".into());
        }
        for dependency in &dependencies {
            if !matches!(
                dependency.dependency_type.as_str(),
                "required" | "incompatible" | "optional" | "embedded"
            ) {
                return Err("未知模组依赖类型".into());
            }
            if matches!(dependency.dependency_type.as_str(), "optional" | "embedded") {
                continue;
            }
            if dependency.project_id.is_none() && dependency.version_id.is_none() {
                return Err("模组依赖缺少项目或版本 ID（文件名依赖不受支持）".into());
            }
            if let Some(id) = &dependency.project_id {
                identifier(id)?;
            }
            if let Some(id) = &dependency.version_id {
                identifier(id)?;
            }
        }
        let version =
            parse_versions(&serde_json::to_vec(&vec![value]).map_err(|_| "依赖元数据无效")?)?
                .into_iter()
                .next()
                .ok_or("依赖没有文件")?;
        self.nodes.insert(
            id.into(),
            Node {
                project: project.into(),
                version,
                dependencies,
                compatible: compatible(value, self.instance, self.loader),
            },
        );
        Ok(id.into())
    }
    async fn exact(&mut self, id: &str, expected: Option<&str>) -> Result<String, String> {
        identifier(id)?;
        if !self.nodes.contains_key(id) {
            let url = reqwest::Url::parse(&format!("{}/version/{id}", self.base))
                .map_err(|_| "依赖地址无效")?;
            let value = self.fetch(url).await?;
            let returned = self.insert(&value, expected)?;
            if returned != id {
                return Err("目录返回的版本 ID 与请求不一致".into());
            }
        }
        if expected.is_some_and(|project| self.nodes[id].project != project) {
            return Err("依赖项目与版本不一致".into());
        }
        Ok(id.into())
    }
    async fn project(&mut self, project: &str) -> Result<Vec<String>, String> {
        identifier(project)?;
        if let Some(ids) = self.lists.get(project) {
            return Ok(ids.clone());
        }
        let loaders = if self.loader == "quilt" {
            vec!["quilt", "fabric"]
        } else {
            vec![self.loader]
        };
        let url = reqwest::Url::parse_with_params(
            &format!("{}/project/{project}/version", self.base),
            &[
                (
                    "game_versions",
                    serde_json::to_string(&[&self.instance.game_version]).unwrap(),
                ),
                ("loaders", serde_json::to_string(&loaders).unwrap()),
            ],
        )
        .map_err(|_| "依赖地址无效")?;
        let values = self.fetch(url).await?;
        let mut values: Vec<_> = values
            .as_array()
            .ok_or("依赖版本列表无效")?
            .iter()
            .filter(|v| compatible(v, self.instance, self.loader))
            .collect();
        let date = |v: &Value| {
            v.get("date_published")
                .or_else(|| v.get("date_created"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        values.sort_by(|a, b| {
            date(b).cmp(&date(a)).then_with(|| {
                a.get("id")
                    .and_then(Value::as_str)
                    .cmp(&b.get("id").and_then(Value::as_str))
            })
        });
        if values.len() > MAX_CANDIDATES {
            self.narrowed = true;
            values.truncate(MAX_CANDIDATES);
        }
        let mut ids = Vec::new();
        for value in values {
            ids.push(self.insert(value, Some(project))?);
        }
        ids.sort_by(|a, b| {
            self.nodes[b]
                .version
                .date_created
                .cmp(&self.nodes[a].version.date_created)
                .then(a.cmp(b))
        });
        ids.dedup();
        self.lists.insert(project.into(), ids.clone());
        Ok(ids)
    }
}

#[derive(Clone, Default)]
struct State {
    selected: BTreeMap<String, String>,
    pending: Vec<DependencyRef>,
    forbidden: Vec<DependencyRef>,
    edges: usize,
    order: Vec<String>,
}

fn forbidden(state: &State, project: &str, version: &str) -> bool {
    state.forbidden.iter().any(|edge| {
        if let Some(id) = &edge.version_id {
            id == version
        } else {
            edge.project_id.as_deref() == Some(project)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Mock {
        base: String,
        requests: Arc<Mutex<Vec<String>>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    impl Mock {
        async fn new(routes: BTreeMap<String, Value>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = requests.clone();
            let task = tokio::spawn(async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0; 2048];
                    while !request.windows(4).any(|s| s == b"\r\n\r\n") && request.len() < 16384 {
                        let n = socket.read(&mut buffer).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..n]);
                    }
                    let request = String::from_utf8_lossy(&request).into_owned();
                    let target = request.split_whitespace().nth(1).unwrap_or("");
                    let path = target.split('?').next().unwrap_or("");
                    let value = routes.get(path);
                    let body = serde_json::to_vec(value.unwrap_or(&Value::Null)).unwrap();
                    let status = if value.is_some() {
                        "200 OK"
                    } else {
                        "404 Not Found"
                    };
                    seen.lock().unwrap().push(target.into());
                    let _ = socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                }
            });
            Self {
                base,
                requests,
                task,
            }
        }
    }
    fn node(id: &str, project: &str, date: &str, dependencies: Value) -> Value {
        serde_json::json!({"id":id,"project_id":project,"date_published":date,"version_number":"same-version-number", "game_versions":["1.21.1"],"loaders":["fabric"],"dependencies":dependencies,"files":[{"url":format!("https://cdn.modrinth.com/{id}.jar"),"filename":format!("{id}.jar"),"size":1,"hashes":{"sha1":"a".repeat(40)}}]})
    }
    fn instance(loader: &str) -> InstanceRecord {
        serde_json::from_value(serde_json::json!({"id":"i","name":"i","gameVersion":"1.21.1","loader":loader,"instanceDir":"fixture","gameDir":"fixture/game","isolated":true,"state":"ready","createdAt":""})).unwrap()
    }
    fn target(value: &Value) -> CatalogVersion {
        parse_versions(&serde_json::to_vec(&[value]).unwrap())
            .unwrap()
            .remove(0)
    }
    fn required(project: &str, version: Option<&str>) -> Value {
        serde_json::json!({"project_id":project,"version_id":version,"dependency_type":"required"})
    }

    #[tokio::test]
    async fn project_choice_backtracks_for_a_later_required_pin_and_honors_user_locks() {
        let a = node(
            "A",
            "PA",
            "2026",
            serde_json::json!([required("PC", Some("C")), required("PB", None)]),
        );
        let c = node(
            "C",
            "PC",
            "2026",
            serde_json::json!([required("PB", Some("B1"))]),
        );
        let b1 = node("B1", "PB", "2025", serde_json::json!([]));
        let b2 = node("B2", "PB", "2026", serde_json::json!([]));
        let mock = Mock::new(BTreeMap::from([
            ("/version/A".into(), a.clone()),
            ("/version/C".into(), c),
            ("/project/PB/version".into(), serde_json::json!([b1, b2])),
        ]))
        .await;
        let target = target(&a);
        let client = reqwest::Client::new();
        let result = resolve(
            &client,
            &mock.base,
            &instance("fabric@1"),
            &[target.clone()],
            &DependencyOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            result
                .iter()
                .map(|v| v.version_id.as_str())
                .collect::<Vec<_>>(),
            ["A", "B1", "C"]
        );
        assert_eq!(mock.requests.lock().unwrap().len(), 3);
        let options = DependencyOptions {
            locks: vec![ProjectVersionLock {
                project_id: "PB".into(),
                version_id: "B2".into(),
            }],
        };
        // A lock's exact endpoint must be verified, not inferred from its ID.
        let b2 = node("B2", "PB", "2026", serde_json::json!([]));
        let c = node(
            "C",
            "PC",
            "2026",
            serde_json::json!([required("PB", Some("B1"))]),
        );
        let b1 = node("B1", "PB", "2025", serde_json::json!([]));
        let locked = Mock::new(BTreeMap::from([
            ("/version/A".into(), a),
            ("/version/B2".into(), b2),
            ("/version/B1".into(), b1),
            ("/version/C".into(), c),
        ]))
        .await;
        assert!(
            resolve(
                &client,
                &locked.base,
                &instance("fabric@1"),
                &[target],
                &options
            )
            .await
            .unwrap_err()
            .contains("冲突")
        );
    }

    #[tokio::test]
    async fn incompatible_version_excludes_only_that_version_not_its_entire_project() {
        let a = node(
            "A",
            "PA",
            "2026",
            serde_json::json!([required("PC", Some("C")), required("PB", None)]),
        );
        let c = node(
            "C",
            "PC",
            "2026",
            serde_json::json!([{"version_id":"B2", "project_id":"PB", "dependency_type":"incompatible"}]),
        );
        let b1 = node("B1", "PB", "2025", serde_json::json!([]));
        let b2 = node("B2", "PB", "2026", serde_json::json!([]));
        let mock = Mock::new(BTreeMap::from([
            ("/version/A".into(), a.clone()),
            ("/version/C".into(), c),
            ("/project/PB/version".into(), serde_json::json!([b2, b1])),
        ]))
        .await;
        let result = resolve(
            &reqwest::Client::new(),
            &mock.base,
            &instance("fabric@1"),
            &[target(&a)],
            &DependencyOptions::default(),
        )
        .await
        .unwrap();
        assert!(result.iter().any(|v| v.version_id == "B1"));
        assert!(!result.iter().any(|v| v.version_id == "B2"));
        assert!(mock.requests.lock().unwrap().len() <= 3);
    }

    #[tokio::test]
    async fn root_lock_and_project_version_identity_fail_closed() {
        let a = node(
            "A",
            "PA",
            "2026",
            serde_json::json!([required("WRONG", Some("B"))]),
        );
        let b = node("B", "PB", "2026", serde_json::json!([]));
        let mock = Mock::new(BTreeMap::from([
            ("/version/A".into(), a.clone()),
            ("/version/B".into(), b),
        ]))
        .await;
        let client = reqwest::Client::new();
        let error = resolve(
            &client,
            &mock.base,
            &instance("fabric@1"),
            &[target(&a)],
            &DependencyOptions::default(),
        )
        .await
        .unwrap_err();
        assert!(error.contains("项目与版本不一致"));
        let options = DependencyOptions {
            locks: vec![ProjectVersionLock {
                project_id: "PA".into(),
                version_id: "OTHER".into(),
            }],
        };
        assert!(
            resolve(
                &client,
                &mock.base,
                &instance("fabric@1"),
                &[target(&a)],
                &options
            )
            .await
            .unwrap_err()
            .contains("根项目")
        );
        let mut changed = target(&a);
        changed.sha1 = "0".repeat(40);
        assert!(
            resolve(
                &client,
                &mock.base,
                &instance("fabric@1"),
                &[changed],
                &DependencyOptions::default()
            )
            .await
            .unwrap_err()
            .contains("文件已变化")
        );
    }

    #[tokio::test]
    async fn supported_catalog_loader_tags_include_forge_neoforge_and_future_names() {
        for loader in [
            "forge",
            "neoforge",
            "cleanroom",
            "legacyfabric",
            "liteloader",
            "optifine",
        ] {
            let mut a = node("A", "PA", "2026", serde_json::json!([]));
            a["loaders"] = serde_json::json!([loader]);
            let mock = Mock::new(BTreeMap::from([("/version/A".into(), a.clone())])).await;
            assert!(
                resolve(
                    &reqwest::Client::new(),
                    &mock.base,
                    &instance(&format!("{loader}@1")),
                    &[target(&a)],
                    &DependencyOptions::default()
                )
                .await
                .is_ok(),
                "{loader}"
            );
        }
        let a = node("A", "PA", "2026", serde_json::json!([]));
        let mock = Mock::new(BTreeMap::from([("/version/A".into(), a.clone())])).await;
        assert!(
            resolve(
                &reqwest::Client::new(),
                &mock.base,
                &instance("quilt@1"),
                &[target(&a)],
                &DependencyOptions::default()
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn candidate_count_ties_and_request_budget_are_deterministic_and_bounded() {
        let a = node("A", "PA", "2026", serde_json::json!([required("PB", None)]));
        let choices: Vec<_> = (0..80)
            .rev()
            .map(|i| node(&format!("B{i:03}"), "PB", "same", serde_json::json!([])))
            .collect();
        let mock = Mock::new(BTreeMap::from([
            ("/version/A".into(), a.clone()),
            ("/project/PB/version".into(), serde_json::json!(choices)),
        ]))
        .await;
        let client = reqwest::Client::new();
        let instance = instance("fabric@1");
        let result = resolve(
            &client,
            &mock.base,
            &instance,
            &[target(&a)],
            &DependencyOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(result[1].version_id, "B000");
        let mut repository = Repository {
            client: &client,
            base: &mock.base,
            instance: &instance,
            loader: "fabric",
            requests: MAX_REQUESTS,
            bytes: 0,
            nodes: BTreeMap::new(),
            lists: BTreeMap::new(),
            narrowed: false,
        };
        assert!(
            repository
                .exact("NEVER", None)
                .await
                .unwrap_err()
                .contains("限额")
        );
        assert_eq!(mock.requests.lock().unwrap().len(), 2);
    }
}

pub(super) async fn resolve(
    client: &reqwest::Client,
    base: &str,
    instance: &InstanceRecord,
    targets: &[CatalogVersion],
    options: &DependencyOptions,
) -> Result<Vec<CatalogVersion>, String> {
    if targets.is_empty() || targets.len() > MAX_PROJECTS || options.locks.len() > MAX_PROJECTS {
        return Err("请选择 1–128 个根版本；版本锁不能超过 128 项".into());
    }
    let loader = loader_of(instance)?;
    let mut repository = Repository {
        client,
        base,
        instance,
        loader,
        requests: 0,
        bytes: 0,
        nodes: BTreeMap::new(),
        lists: BTreeMap::new(),
        narrowed: false,
    };
    let mut locks = BTreeMap::new();
    for lock in &options.locks {
        identifier(&lock.project_id)?;
        identifier(&lock.version_id)?;
        if locks
            .insert(lock.project_id.clone(), lock.version_id.clone())
            .is_some_and(|old| old != lock.version_id)
        {
            return Err(format!("项目 {} 有互相矛盾的版本锁", lock.project_id));
        }
    }
    let mut initial = State::default();
    for target in targets {
        let id = repository.exact(&target.version_id, None).await?;
        let node = &repository.nodes[&id];
        if !node.compatible {
            return Err(format!("根版本 {id} 与游戏版本或加载器不兼容"));
        }
        if node.version.file_name != target.file_name
            || !node.version.sha1.eq_ignore_ascii_case(&target.sha1)
            || node.version.url != target.url
            || node.version.size != target.size
        {
            return Err("模组版本文件已变化，请刷新后重试".into());
        }
        if locks
            .insert(node.project.clone(), id.clone())
            .is_some_and(|old| old != id)
        {
            return Err(format!("根项目 {} 与所选版本/锁定版本冲突", node.project));
        }
        initial.pending.push(DependencyRef {
            project_id: Some(node.project.clone()),
            version_id: Some(id),
            dependency_type: "required".into(),
        });
    }
    initial.pending.reverse();
    let mut stack = vec![initial];
    let mut steps = 0;
    let mut last = String::from("没有满足要求的版本");
    while let Some(mut state) = stack.pop() {
        steps += 1;
        if steps > MAX_STATES {
            return Err("依赖求解达到 4096 搜索状态限额；请减少根项目或明确锁定版本".into());
        }
        let Some(mut edge) = state.pending.pop() else {
            let mut names = BTreeSet::new();
            if state
                .order
                .iter()
                .any(|id| !names.insert(repository.nodes[id].version.file_name.to_lowercase()))
            {
                last = "模组依赖文件名冲突".into();
                continue;
            }
            return Ok(state
                .order
                .iter()
                .map(|id| repository.nodes[id].version.clone())
                .collect());
        };
        if let Some(id) = &edge.version_id {
            repository.exact(id, edge.project_id.as_deref()).await?;
            edge.project_id = Some(repository.nodes[id].project.clone());
        }
        let project = edge.project_id.as_deref().ok_or("模组依赖缺少项目标识")?;
        if let Some(selected) = state.selected.get(project) {
            if edge.version_id.as_ref().is_some_and(|id| id != selected) {
                last = format!(
                    "项目 {project} 的必需版本 {} 与已选 {selected} 冲突",
                    edge.version_id.as_deref().unwrap()
                );
                continue;
            }
            stack.push(state);
            continue;
        }
        let pinned = edge
            .version_id
            .as_ref()
            .or_else(|| locks.get(project))
            .cloned();
        if let (Some(required), Some(locked)) = (&edge.version_id, locks.get(project)) {
            if required != locked {
                last = format!("项目 {project} 要求 {required}，但锁定为 {locked}");
                continue;
            }
        }
        let candidates = if let Some(id) = pinned {
            vec![repository.exact(&id, Some(project)).await?]
        } else {
            repository.project(project).await?
        };
        if candidates.is_empty() {
            last = format!("依赖项目 {project} 没有符合游戏/加载器筛选的版本");
        }
        let mut next_states = Vec::new();
        for id in candidates {
            let node = repository.nodes[&id].clone();
            if !node.compatible || forbidden(&state, project, &id) {
                last = format!("项目 {project} 版本 {id} 与已选版本的兼容/排除约束冲突");
                continue;
            }
            if state.selected.len() >= MAX_PROJECTS
                || state.edges + node.dependencies.len() > MAX_EDGES
            {
                return Err("依赖求解达到项目/依赖边数量限额".into());
            }
            let mut next = state.clone();
            next.selected.insert(project.into(), id.clone());
            next.order.push(id.clone());
            next.edges += node.dependencies.len();
            for mut dependency in node.dependencies {
                match dependency.dependency_type.as_str() {
                    "required" => next.pending.push(dependency),
                    "incompatible" => {
                        if let Some(id) = &dependency.version_id {
                            repository
                                .exact(id, dependency.project_id.as_deref())
                                .await?;
                            dependency.project_id = Some(repository.nodes[id].project.clone());
                        }
                        next.forbidden.push(dependency);
                    }
                    _ => {}
                }
            }
            if next.selected.iter().any(|(p, v)| forbidden(&next, p, v)) {
                last = format!("版本 {id} 声明与已选的必需依赖不兼容");
                continue;
            }
            next_states.push(next);
        }
        // LIFO keeps newest-first selection and deterministic fallback.
        stack.extend(next_states.into_iter().rev());
        if stack.len() > MAX_STATES
            || stack
                .iter()
                .map(|s| s.pending.len() + s.forbidden.len() + s.selected.len())
                .sum::<usize>()
                > MAX_FRONTIER_EDGES
        {
            return Err("依赖求解待搜索状态/累计依赖边数量超限".into());
        }
    }
    Err(format!(
        "依赖求解失败：{last}。仅搜索每项目最新 {MAX_CANDIDATES} 个候选、最多 {MAX_REQUESTS} 次请求{}；没有修改已有模组。",
        if repository.narrowed {
            "（候选列表已按上限截断）"
        } else {
            ""
        }
    ))
}
