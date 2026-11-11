// C++ -> tessera throughput through the C ABI, for the #800 spike numbers.
//
//   capi_bench <stage_dir> <out.tsra> <rows> <batch_rows> <producer_threads> <workers>
//
// Every batch is built before the clock starts, so the timing covers only the bridge and the
// writer: from the first tsra_table_push to the return of tsra_session_seal. Prints one JSON line.
// The native-Rust counterpart is `cargo run --release --example native_bench` (same rows, same
// columns, same writer settings).
#include "listmode.hpp"

#include <sys/resource.h>

#include <chrono>
#include <thread>

using namespace listmode;

int main(int argc, char** argv) {
  if (argc < 7) die("usage: capi_bench <stage_dir> <out.tsra> <rows> <batch_rows> <producer_threads> <workers>");
  const std::string stage = argv[1], out = argv[2];
  const uint64_t rows = std::strtoull(argv[3], nullptr, 10);
  const uint64_t batch_rows = std::strtoull(argv[4], nullptr, 10);
  const int producers = std::atoi(argv[5]);
  const uint32_t workers = static_cast<uint32_t>(std::atoi(argv[6]));

  ArrowSchema schema;
  make_schema(&schema);
  const uint64_t batches = (rows + batch_rows - 1) / batch_rows;
  std::vector<ArrowArray> built(batches);
  for (uint64_t b = 0; b < batches; ++b) {
    const uint64_t first = b * batch_rows;
    make_batch(&schema, first, static_cast<int64_t>(std::min(batch_rows, rows - first)), &built[b]);
  }

  const TsraSessionOptions sopts = session_options(workers);
  TsraSession* session = nullptr;
  check(tsra_session_open(stage.c_str(), &sopts, &session), "tsra_session_open");
  const TsraTableOptions topts = table_options(rows);
  TsraTable* table = nullptr;
  check(tsra_table_begin(session, "events", &schema, &topts, &table), "tsra_table_begin");

  const auto t0 = std::chrono::steady_clock::now();
  std::atomic<uint64_t> next{0};
  std::vector<std::thread> pool;
  for (int p = 0; p < producers; ++p) {
    pool.emplace_back([&] {
      for (uint64_t b; (b = next.fetch_add(1)) < batches;) {
        check(tsra_table_push(table, b, &built[b], &schema), "tsra_table_push");
      }
    });
  }
  for (auto& t : pool) t.join();
  check(tsra_table_end(table), "tsra_table_end");
  char hash[TSRA_HASH_CAPACITY];
  check(tsra_session_seal(session, out.c_str(), hash, sizeof hash), "tsra_session_seal");
  const double secs = std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
  ArrowSchemaRelease(&schema);

  rusage ru{};
  getrusage(RUSAGE_SELF, &ru);
  // 8 + 4 + 4 + 4 + 4 value bytes per row, plus tof's validity bit.
  const double raw_mb = static_cast<double>(rows) * 24.125 / 1e6;
  std::printf(
      "{\"impl\":\"capi\",\"rows\":%llu,\"batch_rows\":%llu,\"producers\":%d,\"workers\":%u,"
      "\"seconds\":%.3f,\"rows_per_s\":%.0f,\"raw_mb_per_s\":%.1f,\"max_rss_mb\":%.1f,"
      "\"content_hash\":\"%s\"}\n",
      static_cast<unsigned long long>(rows), static_cast<unsigned long long>(batch_rows), producers, workers, secs,
      static_cast<double>(rows) / secs, raw_mb / secs, static_cast<double>(ru.ru_maxrss) / 1024.0, hash);
  return 0;
}
