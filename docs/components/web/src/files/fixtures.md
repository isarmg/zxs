<a id="publication-receipt-regression-fixtures"></a>

# 发布收据回归测试数据

这些完整 API 响应来自 `storage-server` HTTP Engine 的合成临时三盘测试池：

- `damage_after_normal_publication_keeps_original_target_met_in_save_wire`
- `recovered_permanent_loss_returns_degraded_save_wire_with_frozen_unavailable_slots`

将 `ZXS_TEST_WIRE_EVIDENCE_DIR` 设为输出目录，并运行 `cargo test -p storage-server --test http_engine save_wire` 可重新生成。ID 和时间戳描述的是合成测试数据，不含凭据。这些文件保留完整、未改动的响应。UI 单元测试仅在内存中修改 `current_health.state`，派生出健康状态对照；该对照明确属于合成数据，并非单独捕获的响应。

测试数据与源码放在一起，使干净检出即可运行 UI 测试，无需依赖被忽略或生成的 `tests/evidence` 产物。
