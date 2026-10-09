# Closure review — các sửa lỗi sau 59be4a9

## Kết luận

**PASS trong phạm vi review tĩnh: không phát hiện blocker mới trong ba bản sửa được giao.** Đây không phải nghiệm thu toàn bộ A–E và không phải tuyên bố các gate đang chạy ở agent cha đã GREEN. Blocker starvation cũ được đóng ở mức logic của delta; kết quả thực thi regression trên snapshot đóng băng vẫn do agent cha cung cấp.

- Repository: `$WORKSPACE/libcram`.
- Base: `59be4a9`; HEAD: `3edfa98d95e039cdaca20e9906de3d10db301f2a`; branch: `main`.
- Tree: `3efa116fcbcdc824d933ca2cfd3b5b588f59e0d7`.
- Đã đọc `git diff 59be4a9..HEAD --` đúng sáu file dưới đây và toàn bộ nội dung hiện tại của chúng. Không đọc implementation dependency ngoài allowlist, không chạy lại Linux/macOS/bindings/soak/package gates, không sửa source.

## 1. Pressure cursor và quota

**Không còn đường reset cursor bởi observation khi pressure >= 2 trong delta được review.**

- `crates/libcram-core/src/runtime.rs:309–315` bỏ observation trước cả ghi `state.sample`, ghi `state.offset` (`316–324`) và gọi migration theo event (`325–342`). Vì vậy hot page không còn kéo cursor quay lại, cũng không tiêu quota dành cho scan.
- Scan cuối tick (`344–359`) nhận quota còn lại, giới hạn allocation scan qua `max_observations` và page scan qua `remaining`. Khi tick luôn ở pressure >= 2, event path không trừ `remaining`; scan được nhận toàn bộ `max_pages`. Không thêm vòng migration ngoài quota.
- `device_update` vẫn xử lý trước intake (`255–273`); health lỗi vẫn bị thu hồi bằng `submit_device_usage(0, 0)` (`224–246`). Fallback scan chỉ chạy khi intake chưa hoàn tất, tránh chi quota lần hai sau migration. Trạng thái `degraded` vẫn suy ra từ lỗi hoặc pressure >= 2 (`248–252`). Khi health phục hồi và pressure < 2, guard mới không chặn event path cũ; scan pressure không chạy. Đây là kiểm tra luồng điều khiển tại runtime, không phải kiểm chứng lại thuật toán pressure/migration ở dependency ngoài phạm vi.
- Test mới `tests/runtime.rs:595–662` đưa hot observation vào từng tick, dùng usage 90/99, allocation 3/5 page, quota 1/2, hot page đầu/cuối; kiểm tra quota mỗi tick và `cram_bytes == 0` cuối vòng. Không lấy `pages_moved` làm số page duy nhất; test có chú thích đúng về rebind DRAM.
- Đã đọc thêm các regression hiện hữu cho quota (`446–482`), pressure không observation (`558–593`), source lỗi liên tục (`665–696`), thu hồi health (`241–259`, `545–556`) và policy bình thường (`261–303`). Đây là assertions trong source, không phải kết quả chạy của reviewer.

## 2. Trusted clock seam và timestamp safety

**Không thấy nới TTL, window, hoặc kiểm tra tuổi sự kiện ở production mặc định.**

- `src/runtime_bridge.rs:130–146`: `Bridge::new` vẫn chụp `Instant::now()` trước `monotonic_ns()`, giữ chuyển đổi theo hướng sớm hơn/bảo thủ như trước. `with_clock` là seam có hợp đồng trusted rõ ràng: cặp tọa độ cùng thời điểm hoặc Instant sớm hơn, clock cùng tốc độ. Hiệu chuẩn sai nằm ngoài hợp đồng này, không được quảng bá là nguồn tùy ý an toàn.
- `238–267`: vẫn loại timestamp tương lai, age >= WINDOW, underflow, event trước token và token quá TTL; không receipt-stamp sự kiện cũ. Runtime vẫn kiểm tra tuổi/future/pre-token độc lập tại `src/runtime.rs:280–288`.
- Lỗi clock đi qua `?`; `device_update` xóa pending/history khi lỗi (`315–321`), runtime tiếp tục thu hồi health theo đường lỗi hiện hữu. Không chuyển lỗi clock thành healthy empty batch.
- `tests/runtime_bridge.rs:17–32,150–156`: event clock và calibration cùng dùng một origin `OnceLock<Instant>`; mỗi cặp lấy nanoseconds từ chính Instant trả về. Fixture không phải hai clock được lấy riêng rồi tăng tolerance để vượt test.
- Đã đọc tests stale/future/pre-token/lifetime/TTL/history/generation và test production empty batch so với lỗi clock (`72–84`). Test production mới chỉ chứng minh ý định kiểm tra clock thật ở empty batch, không phải kiểm chứng conversion của event thật.
- Ghi chú không chặn: comment về hai clock khác nhau và tolerance macOS tại `tests/runtime_bridge.rs:225–233` đã lỗi thời vì test này chuyển sang clock fixture ghép chính xác. Có thể bỏ comment/tolerance trong đợt dọn test sau; delta không thêm tolerance này và kiểm tra receipt-stamping với delay vẫn còn. Không coi đây là lỗi an toàn production.

## 3. SDK nested manifest

**Root manifest là file duy nhất được loại khỏi tập payload hash.**

- `scripts/verify_sdk.py:32–35` so sánh full Path với `prefix / 'MANIFEST.json'`, không còn loại mọi file chỉ vì basename trùng. `docs/MANIFEST.json` và các manifest lồng khác trở thành payload bình thường, phải xuất hiện đúng hash trong mapping.
- `scripts/test_package_sdk.py:13–42` có cả trường hợp nested manifest không được kê khai bị từ chối trước pkg-config/compiler và trường hợp kê khai hợp lệ được tiếp tục.
- Các subprocess trong verifier unit tests được mock. Không dùng bốn lời gọi mock hoặc dòng PASS in ra của fixture làm bằng chứng native C/C++ consumer chạy thật. Archive/native acceptance thực tế thuộc gate riêng của agent cha.

## Giới hạn bằng chứng và acceptance còn mở

- Reviewer chỉ chạy lệnh đọc Git/source/hash; không chạy test/build, không tạo probe, không xác nhận thiết bị CRAM/PEBS thật. RED cũ ba page/quota một còn một page là thông tin agent cha cung cấp, không phải lần tái hiện của reviewer.
- Theo ngữ cảnh bàn giao: B định lượng <1000 ns vẫn FAIL; C universal PEBS/<1% vẫn OPEN; D chưa có real CRAM. Không phân loại lại các thiếu hụt đã biết thành bug mới của delta, cũng không dùng verdict này để đóng chúng.
- Không yêu cầu giữ lỗi FAIL cũ sau khi source đã sửa; verdict này gắn với đúng HEAD mới và đúng sáu file.
- Chỉ ghi artifact `target/review-fixes.md`; không sửa source, Git index hoặc metadata. Trạng thái Git lúc mở review sạch. Kiểm tra identity/hash cuối và đọc lại report được thực hiện trước handoff; nếu lệch snapshot thì verdict không được chuyển giao.

## Manifest SHA-256 của sáu file được review

```text
4bcb3d994166650893fbcf1e5bdef7caa6cdb8692f45e91bb09644fb6d4a6907  crates/libcram-core/src/runtime.rs
9f92a92f73e40d0facd098181ecfdc91fc048c7aabdb48b4021bc6f787810921  crates/libcram-core/src/runtime_bridge.rs
94e8d4f583b2243ac0549f78ca9d08b4f93fb32efa6cd88da3c432e59e3c9c5c  crates/libcram-core/tests/runtime.rs
1eec83ffa14b2d65e457fb973791aee8d84b84ed7b09f7f9bf29054b5ded6a37  crates/libcram-core/tests/runtime_bridge.rs
dd15523b3f6b032dd7532b360c80e9bce300a40c8835e4ab99962e5b56cb00ad  scripts/verify_sdk.py
1566f406cf613e96860b8e2652cb1f3e531350c84b8629e4a7dded6cd95773d4  scripts/test_package_sdk.py
```
