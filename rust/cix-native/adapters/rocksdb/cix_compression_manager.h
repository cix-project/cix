// SPDX-License-Identifier: MIT
#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>

#include <rocksdb/advanced_compression.h>

namespace rocksdb {

/**
 * @brief Creates the experimental CIX manager pinned to RocksDB v10.10.1
 * (commit 4595a5e).
 *
 * @param max_block_bytes Per-block admission cap, from 1 through 128 MiB.
 * @return A manager on success, or `nullptr` for an invalid cap or allocation
 * failure.
 */
std::shared_ptr<CompressionManager> NewCixCompressionManagerV1(
    size_t max_block_bytes = 64U << 20);

/** @brief Independently loaded counters for a CIX compression manager. */
struct CixCompressionManagerStatsV1 {
  uint64_t encode_attempts;  ///< Compression callback attempts.
  uint64_t cix_blocks;      ///< Blocks emitted with the CIX compression type.
  uint64_t raw_fallbacks;   ///< Blocks emitted without CIX compression.
  uint64_t decode_attempts; ///< Decompression callback attempts.
};

/**
 * @brief Obtains a counter snapshot from a CIX manager.
 *
 * @param manager Manager to inspect; the caller retains its shared ownership.
 * @param out Caller-owned destination written only when this function returns
 * `true`.
 * @return `true` for a compatible CIX manager and non-null @p out; otherwise
 * `false`.
 *
 * The counters are atomic individual loads and may change while the manager is
 * in use, so this is not a single coherent point-in-time aggregate.
 */
bool GetCixCompressionManagerStatsV1(const std::shared_ptr<CompressionManager>& manager,
                                     CixCompressionManagerStatsV1* out);

}  // namespace rocksdb
