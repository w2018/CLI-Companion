//! ServiceManager：actor 表的统一入口（RPC 层只与本模块交互）

use crate::actor::{spawn_actor, ActorCmd, ActorHandle, ActorMap, SharedState};
use crate::dirs::DataDirs;
use cli_companion_domain::{RuntimeState, ServiceDefinition, ServiceId};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;

pub struct ServiceManager {
    actors: Mutex<ActorMap>,
    dirs: DataDirs,
}

impl ServiceManager {
    pub fn new(dirs: DataDirs) -> Self {
        Self { actors: Mutex::new(HashMap::new()), dirs }
    }

    /// 获取或创建 actor（幂等）
    pub fn ensure_actor(&self, id: ServiceId) -> ActorHandle {
        let mut map = self.actors.lock().unwrap();
        map.entry(id).or_insert_with(|| spawn_actor(id, self.dirs.clone())).clone()
    }

    /// 移除 actor（关闭邮箱；残留子进程由 KILL_ON_JOB_CLOSE 兜底）
    pub fn remove_actor(&self, id: &ServiceId) {
        let mut map = self.actors.lock().unwrap();
        map.remove(id);
    }

    /// 启动服务
    pub async fn start(&self, def: &ServiceDefinition) -> Result<(), String> {
        let handle = self.ensure_actor(def.id);
        let (tx, rx) = oneshot::channel();
        handle
            .tx
            .send(ActorCmd::Start { def: Box::new(def.clone()), reply: tx })
            .await
            .map_err(|_| "actor 已退出".to_string())?;
        rx.await.map_err(|_| "actor 未响应".to_string())?
    }

    /// 停止服务
    pub async fn stop(&self, id: ServiceId) -> Result<(), String> {
        let handle = { self.actors.lock().unwrap().get(&id).cloned() };
        match handle {
            Some(h) => {
                let (tx, rx) = oneshot::channel();
                h.tx
                    .send(ActorCmd::Stop { reply: tx })
                    .await
                    .map_err(|_| "actor 已退出".to_string())?;
                rx.await.map_err(|_| "actor 未响应".to_string())?
            }
            None => Ok(()), // 无 actor 即视为已停止
        }
    }

    /// 重启服务（先完全停止，再以新参数启动）
    pub async fn restart(&self, def: &ServiceDefinition) -> Result<(), String> {
        let handle = self.ensure_actor(def.id);
        let (tx, rx) = oneshot::channel();
        handle
            .tx
            .send(ActorCmd::Restart { def: Box::new(def.clone()), reply: tx })
            .await
            .map_err(|_| "actor 已退出".to_string())?;
        rx.await.map_err(|_| "actor 未响应".to_string())?
    }

    /// 读取单个服务运行时状态
    #[allow(dead_code)] // 供集成测试使用
    pub fn runtime_of(&self, id: &ServiceId) -> Option<RuntimeState> {
        let map = self.actors.lock().unwrap();
        map.get(id).and_then(|h| h.state.lock().ok().map(|s| s.clone()))
    }

    /// 全部运行时状态
    pub fn all_runtimes(&self) -> HashMap<ServiceId, RuntimeState> {
        let map = self.actors.lock().unwrap();
        map.iter()
            .filter_map(|(id, h)| h.state.lock().ok().map(|s| (*id, s.clone())))
            .collect()
    }

    /// 停止所有服务（daemon 关闭时）—— 并行发出停止命令，统一等待完成
    /// （串行等待会让"每个服务最长 25 秒"叠加，daemon 关闭极慢）
    pub async fn stop_all(&self) {
        let handles: Vec<ActorHandle> = {
            let map = self.actors.lock().unwrap();
            map.values().cloned().collect()
        };
        let mut waiters = Vec::with_capacity(handles.len());
        for h in handles {
            let (tx, rx) = oneshot::channel();
            if h.tx.send(ActorCmd::Stop { reply: tx }).await.is_ok() {
                waiters.push(rx);
            }
        }
        // 并行等待全部停止完成
        for rx in waiters {
            let _ = rx.await;
        }
    }

    /// 配置变更后同步 actor 表：新增创建、删除移除
    pub fn sync_actors(&self, ids: &[ServiceId]) {
        let mut map = self.actors.lock().unwrap();
        // 新增
        for id in ids {
            map.entry(*id)
                .or_insert_with(|| spawn_actor(*id, self.dirs.clone()));
        }
        // 移除已删除的服务 actor
        let to_remove: Vec<ServiceId> = map
            .keys()
            .filter(|k| !ids.contains(k))
            .copied()
            .collect();
        for id in to_remove {
            map.remove(&id);
        }
    }

    /// 某服务的共享状态（测试用）
    #[allow(dead_code)] // 供集成测试使用
    pub fn state_of(&self, id: &ServiceId) -> Option<SharedState> {
        self.actors.lock().unwrap().get(id).map(|h| h.state.clone())
    }
}
