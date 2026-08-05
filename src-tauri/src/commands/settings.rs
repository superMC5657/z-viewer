//! 用户设置：缓存等级与文件夹首图队列深度（纯数据，无命令逻辑）

/// 用户设置
/// - cache_level：0=关闭（不缓存不预取）1=开启（前后各 1）2=高等级（前 1 后 3）
/// - folder_first_depth：文件夹首图队列深度（默认 1；跨文件夹跳转缓存）
#[derive(serde::Serialize, Clone, Copy)]
pub struct AppSettings {
    pub cache_level: usize,
    pub folder_first_depth: usize,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            cache_level: 1,
            folder_first_depth: 1,
        }
    }
}

impl AppSettings {
    /// 邻居预取窗口：0=不预取；1=前1后1；2(高)=前1后3
    pub fn neighbor_window(&self) -> (usize, usize) {
        match self.cache_level {
            0 => (0, 0),
            1 => (1, 1),
            _ => (1, 3), // 高等级：前 1 后 3
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.cache_level > 0
    }
}
