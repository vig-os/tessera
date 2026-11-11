// #802 evidence from C++: N std::threads push one table's batches in a scheduler-chosen order.
//
//   capi_threads <stage_base_dir>
//
// With an explicit sequence key (the batch's own index) the content hash is the same for
// N in {1, 4, 16} over several shuffled schedules. With the key taken in arrival order, the hash
// follows the scheduler. A third set claims keys in increasing order (G4TaskRunManager's shape)
// through a reorder window far smaller than the batch count, so pushes block on the window, and
// still seals the in-order hash. Exits non-zero if any claim fails.
//
// The shuffled runs set the window to the batch count: with an arbitrary assignment a blocking
// window can deadlock (src/order.rs), so only the claimed runs use a small one.
#include "listmode.hpp"

#include <algorithm>
#include <map>
#include <random>
#include <set>
#include <thread>

using namespace listmode;

namespace {

constexpr uint64_t kBatches = 48;
constexpr int64_t kBatchRows = 3001;
constexpr uint64_t kClaimedBatches = 200;
constexpr int64_t kClaimedBatchRows = 997;
constexpr uint32_t kClaimedWindow = 4;

enum class Key { Explicit, Arrival };
enum class Assign { Shuffled, Claimed };

std::string run(const std::string& stage, int threads, uint32_t seed, Key key, Assign assign) {
  const bool claimed = assign == Assign::Claimed;
  const uint64_t batches = claimed ? kClaimedBatches : kBatches;
  const int64_t batch_rows = claimed ? kClaimedBatchRows : kBatchRows;
  ArrowSchema schema;
  make_schema(&schema);
  const TsraSessionOptions sopts = session_options(2);
  TsraSession* session = nullptr;
  check(tsra_session_open(stage.c_str(), &sopts, &session), "tsra_session_open");
  TsraTableOptions topts = table_options(batches * static_cast<uint64_t>(batch_rows));
  topts.reorder_window = claimed ? kClaimedWindow : static_cast<uint32_t>(batches);
  TsraTable* table = nullptr;
  check(tsra_table_begin(session, "events", &schema, &topts, &table), "tsra_table_begin");

  std::vector<uint64_t> queue(batches);
  for (uint64_t b = 0; b < batches; ++b) queue[b] = b;
  if (!claimed) std::shuffle(queue.begin(), queue.end(), std::mt19937(seed));
  std::atomic<size_t> taken{0};
  std::atomic<uint64_t> arrival{0};
  std::vector<std::thread> pool;
  for (int w = 0; w < threads; ++w) {
    pool.emplace_back([&, w] {
      std::mt19937 jitter(seed * 7919u + static_cast<uint32_t>(w));
      for (size_t k; (k = taken.fetch_add(1)) < queue.size();) {
        const uint64_t b = queue[k];
        ArrowArray array;
        make_batch(&schema, b * static_cast<uint64_t>(batch_rows), batch_rows, &array);
        for (unsigned y = jitter() % 4; y > 0; --y) std::this_thread::yield();
        const uint64_t seq = key == Key::Explicit ? b : arrival.fetch_add(1);
        check(tsra_table_push(table, seq, &array, &schema), "tsra_table_push");
      }
    });
  }
  for (auto& t : pool) t.join();
  check(tsra_table_end(table), "tsra_table_end");
  char hash[TSRA_HASH_CAPACITY];
  const std::string out = stage + ".tsra";
  check(tsra_session_seal(session, out.c_str(), hash, sizeof hash), "tsra_session_seal");
  ArrowSchemaRelease(&schema);
  return hash;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc < 2) die("usage: capi_threads <stage_base_dir>");
  const std::string base = argv[1];
  int run_no = 0;
  auto stage = [&] { return base + "/run" + std::to_string(run_no++); };

  const std::string reference = run(stage(), 1, 0, Key::Explicit, Assign::Shuffled);
  std::printf("reference (N=1, seed 0)  %s\n", reference.c_str());
  bool ok = true;
  for (int n : {1, 4, 16}) {
    for (uint32_t seed = 1; seed <= 3; ++seed) {
      const std::string h = run(stage(), n, seed, Key::Explicit, Assign::Shuffled);
      const bool same = h == reference;
      ok = ok && same;
      std::printf("explicit N=%-2d seed %u   %s %s\n", n, seed, h.c_str(), same ? "==" : "!= REFERENCE");
    }
  }
  std::set<std::string> arrival;
  for (int n : {1, 4, 16}) {
    for (uint32_t seed = 1; seed <= 3; ++seed) {
      const std::string h = run(stage(), n, seed, Key::Arrival, Assign::Shuffled);
      const bool differs = h != reference;
      ok = ok && differs;
      arrival.insert(h);
      std::printf("arrival  N=%-2d seed %u   %s %s\n", n, seed, h.c_str(), differs ? "!= reference" : "== REFERENCE (unexpected)");
    }
  }
  std::printf("arrival order: %zu distinct hashes over 9 runs\n", arrival.size());
  if (arrival.size() < 2) ok = false;
  // Claimed keys through a small window: N=1 is the in-order reference, every N must match it.
  const std::string claimed_ref = run(stage(), 1, 0, Key::Explicit, Assign::Claimed);
  std::printf("claimed  reference (N=1)  %s (window %u, %llu batches)\n", claimed_ref.c_str(), kClaimedWindow,
              static_cast<unsigned long long>(kClaimedBatches));
  for (int n : {4, 16}) {
    for (uint32_t seed = 1; seed <= 2; ++seed) {
      const std::string h = run(stage(), n, seed, Key::Explicit, Assign::Claimed);
      const bool same = h == claimed_ref;
      ok = ok && same;
      std::printf("claimed  N=%-2d seed %u   %s %s\n", n, seed, h.c_str(), same ? "==" : "!= REFERENCE");
    }
  }
  if (!ok) die("determinism claims failed");
  std::printf("OK: explicit-key hashes are identical for N in {1,4,16}, shuffled and claimed; arrival order is not\n");
  return 0;
}
