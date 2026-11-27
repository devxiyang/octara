//! Built-in component labels, inherited through Octara context.

use octara::prelude::BuildCx;

/// The strings used by Kit when the application does not supply its own label.
///
/// Provide a locale with `build.provide(KitLocale::zh_cn())`. Override individual labels
/// using struct update syntax, for example `KitLocale { close: "Done", ..KitLocale::en() }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KitLocale {
    pub copy: &'static str,
    pub copied: &'static str,
    pub search: &'static str,
    pub close: &'static str,
    pub loading: &'static str,
    pub clear_search: &'static str,
    pub dismiss_notification: &'static str,
    pub search_commands: &'static str,
    pub breadcrumb: &'static str,
    /// `"{item}"` is replaced with the chip label.
    pub remove_item: &'static str,
    /// `"{item}"` is replaced with the picker label.
    pub search_item: &'static str,
    pub no_results: &'static str,
    pub commands: &'static str,
    pub show_password: &'static str,
    pub hide_password: &'static str,
    pub value: &'static str,
    pub progress: &'static str,
    pub resize: &'static str,
    /// `"{item}"` is replaced with the column label.
    pub sort_column: &'static str,
    /// `"{item}"` is replaced with the column label.
    pub resize_column: &'static str,
    pub note: &'static str,
    pub tip: &'static str,
    pub important: &'static str,
    pub warning: &'static str,
    pub caution: &'static str,
}

impl KitLocale {
    /// The English default.
    pub const fn en() -> Self {
        Self {
            copy: "Copy",
            copied: "Copied",
            search: "Search",
            close: "Close",
            loading: "Loading",
            clear_search: "Clear search",
            dismiss_notification: "Dismiss notification",
            search_commands: "Search commands",
            breadcrumb: "Breadcrumb",
            remove_item: "Remove {item}",
            search_item: "Search {item}",
            no_results: "No results",
            commands: "Commands",
            show_password: "Show password",
            hide_password: "Hide password",
            value: "Value",
            progress: "Progress",
            resize: "Resize",
            sort_column: "Sort by {item}",
            resize_column: "Resize {item} column",
            note: "Note",
            tip: "Tip",
            important: "Important",
            warning: "Warning",
            caution: "Caution",
        }
    }

    /// Simplified Chinese labels for Mainland China.
    pub const fn zh_cn() -> Self {
        Self {
            copy: "复制",
            copied: "已复制",
            search: "搜索",
            close: "关闭",
            loading: "正在加载",
            clear_search: "清除搜索",
            dismiss_notification: "关闭通知",
            search_commands: "搜索命令",
            breadcrumb: "面包屑导航",
            remove_item: "移除{item}",
            search_item: "搜索{item}",
            no_results: "无搜索结果",
            commands: "命令",
            show_password: "显示密码",
            hide_password: "隐藏密码",
            value: "值",
            progress: "进度",
            resize: "调整大小",
            sort_column: "按{item}排序",
            resize_column: "调整{item}列的宽度",
            note: "备注",
            tip: "提示",
            important: "重要",
            warning: "警告",
            caution: "注意",
        }
    }

    /// Reads the nearest provided locale, falling back to English.
    pub fn get(build: &BuildCx<'_>) -> Self {
        build.context().unwrap_or_default()
    }

    /// Formats the search label for a named picker.
    pub fn search_label(&self, item: &str) -> String {
        self.search_item.replace("{item}", item)
    }
}

impl Default for KitLocale {
    fn default() -> Self {
        Self::en()
    }
}
