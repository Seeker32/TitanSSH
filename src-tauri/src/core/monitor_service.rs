use crate::models::monitor::{MonitorSnapshot, TaskInfo, TaskStatus};
use crate::models::session::TaskStatusEvent;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

/// 监控任务句柄，包含任务元数据和关闭信号
struct MonitorTaskHandle {
    /// 任务基本信息（ID、类型、状态等）
    task_info: TaskInfo,
    /// 关闭标志，设置为 true 时通知工作线程退出
    shutdown: Arc<AtomicBool>,
}

/// 独立监控服务
///
/// 负责管理所有监控任务的生命周期，包括启动、停止和状态查询。
/// 通过 Arc<Mutex<...>> 保证多线程安全访问。
pub struct MonitorService {
    /// 活跃监控任务的 HashMap，键为 task_id
    tasks: Arc<Mutex<HashMap<String, MonitorTaskHandle>>>,
    /// 最新监控快照的 HashMap，键为 session_id
    snapshots: Arc<Mutex<HashMap<String, MonitorSnapshot>>>,
}

impl MonitorService {
    /// 创建新的监控服务实例
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            snapshots: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 为指定会话启动监控任务
    ///
    /// 生成唯一 task_id，创建 TaskInfo（初始状态为 Pending），
    /// 启动后台工作线程定期采集快照并推送事件。
    ///
    /// # 参数
    /// - `session_id`: 关联的会话 ID
    /// - `app`: Tauri 应用句柄，用于派发事件
    ///
    /// # 返回
    /// 新建的 TaskInfo，包含 task_id 和初始状态
    pub fn start_monitoring(&self, session_id: String, app: AppHandle) -> TaskInfo {
        // 生成唯一任务 ID
        let task_id = Uuid::new_v4().to_string();

        // 创建任务信息，初始状态为 Pending
        let task_info = TaskInfo {
            task_id: task_id.clone(),
            task_type: "monitor".to_string(),
            session_id: Some(session_id.clone()),
            status: TaskStatus::Pending,
            created_at: chrono::Utc::now().timestamp_millis(),
        };

        // 创建关闭标志
        let shutdown = Arc::new(AtomicBool::new(false));

        // 将任务句柄注册到 HashMap
        {
            let mut tasks = self.tasks.lock().unwrap();
            tasks.insert(
                task_id.clone(),
                MonitorTaskHandle {
                    task_info: task_info.clone(),
                    shutdown: shutdown.clone(),
                },
            );
        }

        // 克隆共享状态引用，供工作线程使用
        let tasks_ref = Arc::clone(&self.tasks);
        let snapshots_ref = Arc::clone(&self.snapshots);

        // 派发 Pending 状态事件
        emit_task_status(&app, &task_id, TaskStatus::Pending, None);

        // 启动后台监控工作线程
        thread::spawn(move || {
            // 更新任务状态为 Running
            update_task_status(&tasks_ref, &task_id, TaskStatus::Running);
            emit_task_status(&app, &task_id, TaskStatus::Running, None);

            // 模拟计数器，用于生成变化的占位数据
            let mut tick: u64 = 0;

            // 主循环：定期采集快照并推送事件
            while !shutdown.load(Ordering::Relaxed) {
                // 生成占位监控快照（MVP 阶段使用模拟数据）
                let snapshot = generate_placeholder_snapshot(&session_id, tick);

                // 更新最新快照缓存
                {
                    let mut snapshots = snapshots_ref.lock().unwrap();
                    snapshots.insert(session_id.clone(), snapshot.clone());
                }

                // 推送 monitor:snapshot 事件到前端
                if let Err(err) = app.emit("monitor:snapshot", &snapshot) {
                    // 事件派发失败视为监控任务异常，派发 Failed 状态后退出
                    update_task_status(&tasks_ref, &task_id, TaskStatus::Failed);
                    emit_task_status(
                        &app,
                        &task_id,
                        TaskStatus::Failed,
                        Some(format!("监控快照推送失败: {err}")),
                    );
                    return;
                }

                tick = tick.wrapping_add(1);

                // 每 2 秒采集一次，分 20 次 100ms 检查关闭标志
                for _ in 0..20 {
                    if shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }

            // 工作线程正常退出时更新任务状态为 Done
            update_task_status(&tasks_ref, &task_id, TaskStatus::Done);
            emit_task_status(&app, &task_id, TaskStatus::Done, None);
        });

        task_info
    }

    /// 停止指定任务 ID 对应的监控任务
    ///
    /// 设置关闭标志，通知工作线程退出，并从任务 HashMap 中移除句柄。
    ///
    /// # 参数
    /// - `task_id`: 要停止的监控任务 ID
    pub fn stop_monitoring(&self, task_id: &str) {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(handle) = tasks.remove(task_id) {
            // 通知工作线程退出
            handle.shutdown.store(true, Ordering::Relaxed);
        }
    }

    /// 获取指定会话的最新监控快照
    ///
    /// # 参数
    /// - `session_id`: 会话 ID
    ///
    /// # 返回
    /// 若存在缓存快照则返回 Some(MonitorSnapshot)，否则返回 None
    pub fn get_monitor_status(&self, session_id: &str) -> Option<MonitorSnapshot> {
        let snapshots = self.snapshots.lock().unwrap();
        snapshots.get(session_id).cloned()
    }
}

/// 生成占位监控快照（MVP 阶段使用模拟数据）
///
/// 使用 tick 计数器生成随时间变化的模拟指标，
/// 确保任务生命周期和事件推送逻辑可正常验证。
///
/// # 参数
/// - `session_id`: 关联的会话 ID
/// - `tick`: 当前采集轮次，用于生成变化数据
fn generate_placeholder_snapshot(session_id: &str, tick: u64) -> MonitorSnapshot {
    // 使用简单的正弦波模拟 CPU 使用率波动（20% ~ 60%）
    let cpu_usage = 20.0 + (tick as f64 * 0.3).sin().abs() * 40.0;
    // 内存使用率缓慢增长后回落（40% ~ 70%）
    let memory_usage = 40.0 + (tick as f64 * 0.1).sin().abs() * 30.0;
    // 磁盘使用率相对稳定（50% ~ 65%）
    let disk_usage = 50.0 + (tick as f64 * 0.05).sin().abs() * 15.0;

    MonitorSnapshot {
        session_id: session_id.to_string(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        cpu_usage,
        memory_usage,
        disk_usage,
    }
}

/// 更新任务 HashMap 中指定任务的状态
///
/// # 参数
/// - `tasks`: 任务 HashMap 的共享引用
/// - `task_id`: 要更新的任务 ID
/// - `status`: 新的任务状态
fn update_task_status(
    tasks: &Arc<Mutex<HashMap<String, MonitorTaskHandle>>>,
    task_id: &str,
    status: TaskStatus,
) {
    let mut tasks = tasks.lock().unwrap();
    if let Some(handle) = tasks.get_mut(task_id) {
        handle.task_info.status = status;
    }
}

/// 派发任务状态变更事件到前端
///
/// # 参数
/// - `app`: Tauri 应用句柄
/// - `task_id`: 任务 ID
/// - `status`: 新的任务状态
/// - `message`: 可选的附加消息（如错误详情）
fn emit_task_status(
    app: &AppHandle,
    task_id: &str,
    status: TaskStatus,
    message: Option<String>,
) {
    let _ = app.emit(
        "task:status",
        TaskStatusEvent {
            task_id: task_id.to_string(),
            status,
            message,
        },
    );
}

/// 监控任务状态机，用于验证状态流转合法性（纯逻辑，不依赖 Tauri 运行时）
///
/// 合法流转 DAG：Pending → Running → Done
///                                  → Failed
#[cfg_attr(not(test), allow(dead_code))]
pub struct TaskStateMachine {
    /// 当前任务状态
    pub status: TaskStatus,
}

impl TaskStateMachine {
    /// 创建初始状态为 Pending 的状态机
    pub fn new() -> Self {
        Self {
            status: TaskStatus::Pending,
        }
    }

    /// 尝试将状态流转到目标状态，返回是否成功
    ///
    /// 合法流转：Pending → Running，Running → Done，Running → Failed
    /// 其余流转均视为非法，返回 false
    pub fn transition(&mut self, next: TaskStatus) -> bool {
        let valid = matches!(
            (&self.status, &next),
            (TaskStatus::Pending, TaskStatus::Running)
                | (TaskStatus::Running, TaskStatus::Done)
                | (TaskStatus::Running, TaskStatus::Failed)
        );
        if valid {
            self.status = next;
        }
        valid
    }

    /// 判断当前状态是否为终止状态（Done 或 Failed）
    pub fn is_terminal(&self) -> bool {
        matches!(self.status, TaskStatus::Done | TaskStatus::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashSet;

    /// 生成合法的状态流转序列策略：Pending → Running → Done/Failed
    ///
    /// 返回一个包含完整合法流转步骤的 Vec<TaskStatus>
    fn valid_transition_sequence() -> impl Strategy<Value = Vec<TaskStatus>> {
        prop::bool::ANY.prop_map(|use_done| {
            vec![
                TaskStatus::Running,
                if use_done {
                    TaskStatus::Done
                } else {
                    TaskStatus::Failed
                },
            ]
        })
    }

    /// 生成非法状态流转策略（从终止状态继续流转）
    fn invalid_transition_after_terminal() -> impl Strategy<Value = (TaskStatus, TaskStatus)> {
        let terminal = prop_oneof![Just(TaskStatus::Done), Just(TaskStatus::Failed)];
        let next = prop_oneof![
            Just(TaskStatus::Pending),
            Just(TaskStatus::Running),
            Just(TaskStatus::Done),
            Just(TaskStatus::Failed),
        ];
        (terminal, next)
    }

    proptest! {
        /// **验证: 需求 7.1**
        ///
        /// Property 9: 监控任务具备完整状态流转
        ///
        /// 验证合法流转序列（Pending → Running → Done/Failed）全部成功，
        /// 且最终状态为终止状态
        #[test]
        fn prop_valid_state_transitions_succeed(transitions in valid_transition_sequence()) {
            let mut sm = TaskStateMachine::new();
            // 初始状态必须为 Pending
            prop_assert!(sm.status == TaskStatus::Pending, "初始状态应为 Pending");

            for next in transitions {
                let result = sm.transition(next);
                prop_assert!(result, "合法流转应当成功");
            }

            // 经过完整合法序列后，最终状态必须为终止状态
            prop_assert!(sm.is_terminal(), "完整流转后状态应为 Done 或 Failed");
        }

        /// **验证: 需求 7.1**
        ///
        /// Property 9: 非法状态流转被拒绝
        ///
        /// 验证从终止状态（Done/Failed）发起的任何流转均被拒绝
        #[test]
        fn prop_invalid_transitions_from_terminal_are_rejected(
            (terminal, next) in invalid_transition_after_terminal()
        ) {
            let mut sm = TaskStateMachine {
                status: terminal.clone(),
            };
            let result = sm.transition(next);
            prop_assert!(!result, "从终止状态发起的流转应当被拒绝");
            // 状态不应发生变化
            prop_assert!(sm.status == terminal, "拒绝流转后状态不应改变");
        }

        /// **验证: 需求 7.1**
        ///
        /// Property 9: 跳过 Running 直接到终止状态的流转被拒绝
        ///
        /// 验证 Pending 不能直接跳转到 Done 或 Failed
        #[test]
        fn prop_pending_cannot_skip_running(
            terminal in prop_oneof![Just(TaskStatus::Done), Just(TaskStatus::Failed)]
        ) {
            let mut sm = TaskStateMachine::new();
            let result = sm.transition(terminal);
            prop_assert!(!result, "Pending 不能直接跳转到终止状态");
            prop_assert_eq!(sm.status, TaskStatus::Pending, "状态应保持 Pending");
        }

        /// **验证: 需求 7.1**
        ///
        /// Property 9: 多个监控任务的 task_id 唯一
        ///
        /// 生成 1..=10 个任务，验证所有 task_id 互不相同
        #[test]
        fn prop_task_ids_are_unique(n in 1usize..=10usize) {
            // 模拟生成 n 个任务 ID（与 MonitorService 使用相同的 UUID v4 生成方式）
            let ids: Vec<String> = (0..n).map(|_| uuid::Uuid::new_v4().to_string()).collect();
            let unique: HashSet<&String> = ids.iter().collect();
            prop_assert_eq!(
                unique.len(),
                ids.len(),
                "所有 task_id 必须唯一，发现重复: {:?}",
                ids
            );
        }

        /// **验证: 需求 7.1**
        ///
        /// Property 9: Running → Done 和 Running → Failed 均为合法终止流转
        ///
        /// 验证两种终止路径都能正常完成
        #[test]
        fn prop_running_can_transition_to_done_or_failed(
            use_done in prop::bool::ANY
        ) {
            let mut sm = TaskStateMachine::new();
            // Pending → Running
            prop_assert!(sm.transition(TaskStatus::Running));
            // Running → Done 或 Running → Failed
            let terminal = if use_done { TaskStatus::Done } else { TaskStatus::Failed };
            prop_assert!(sm.transition(terminal));
            prop_assert!(sm.is_terminal());
        }
    }
}
