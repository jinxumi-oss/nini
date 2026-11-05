//! nini 的库入口：把二进制里的模块暴露给集成测试。
//!
//! # 为什么有这个文件
//!
//! nini-cli 原本**只有一个 bin 目标**，集成测试拿不到它的内部模块。R5 的
//! provider 薄壳测试需要**直接**调用 `build_provider`（而不是通过进程去猜
//! 程序的行为：进程一退出就 `exit(2)`，测不出「拒绝的消息里点名了哪个
//! provider」），所以这里开一个 lib 目标，把 `provider_factory` 暴露出来。
//!
//! # 这个 lib 目标不改变程序的行为
//!
//! bin 目标**不链接**它：可执行文件里仍然是 `main.rs` 自己那份
//! `provider_factory`。两份是同一份源码，编译出来的东西一样。
//!
//! # 为什么不把 `provider_factory` 整体搬进 lib
//!
//! `main.rs` 与 `app.rs` / `demo.rs` 之间有一堆 `crate::` 交叉引用，搬模块
//! 就是一次重构。R5 不需要重构。

// 这个 lib 只是 bin 的镜像：`main.rs` 里用到 `provider_factory` 的那些模块
// （`app` / `demo` …）不在这个 lib 里，所以它的一部分公开项在这里「没人用」。
// 在 bin 目标里它们是有用的（`dead_code` 分析是按 crate 做的）。
#![allow(dead_code)]

pub mod provider_factory;
