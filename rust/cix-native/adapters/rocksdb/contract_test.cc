// SPDX-License-Identifier: MIT
#include "cix_compression_manager.h"
#include <rocksdb/db.h>
#include <rocksdb/options.h>
#include <rocksdb/table_properties.h>
#include <rocksdb/table.h>
#include <array>
#include <filesystem>
#include <iostream>
#include <map>
#include <string>

namespace {

bool require(bool condition, const char* message) {
  if (!condition) std::cerr << "RocksDB contract failure: " << message << '\n';
  return condition;
}

}  // namespace

int main() {
  auto path = (std::filesystem::temp_directory_path() / "cix-rocksdb-v10101-contract").string();
  std::filesystem::remove_all(path);
  auto manager = rocksdb::NewCixCompressionManagerV1();
  if (!require(manager != nullptr, "create default CIX compression manager")) return 1;
  if (!require(!rocksdb::NewCixCompressionManagerV1((128U << 20) + 1), "reject manager limit above 128 MiB")) return 1;
  rocksdb::Options options; options.create_if_missing = true;
  options.compression = rocksdb::kCustomCompression80; options.compression_manager = manager;
  rocksdb::BlockBasedTableOptions table_options; table_options.format_version = 7;
  options.table_factory.reset(rocksdb::NewBlockBasedTableFactory(table_options));
  options.write_buffer_size = 64U << 10;
  rocksdb::DB* db = nullptr;
  const auto create_open = rocksdb::DB::Open(options, path, &db);
  if (!require(create_open.ok() && db != nullptr, "create CIX database")) return 1;
  std::string repetitive(256U << 10, 'A'), random(256U << 10, '\0'); uint32_t state = 0x12345678;
  for (char& byte : random) { state = state * 1664525U + 1013904223U; byte = static_cast<char>(state >> 24); }
  if (!require(db->Put(rocksdb::WriteOptions(), "repetitive", repetitive).ok(), "write repetitive value")) return 1;
  if (!require(db->Put(rocksdb::WriteOptions(), "random", random).ok(), "write random value")) return 1;
  rocksdb::FlushOptions flush; flush.wait = true;
  if (!require(db->Flush(flush).ok(), "flush CIX SST")) return 1;
  rocksdb::TablePropertiesCollection properties;
  if (!require(db->GetPropertiesOfAllTables(&properties).ok(), "read table properties")) return 1;
  bool cix_sst = false;
  for (const auto& entry : properties) cix_sst |= entry.second->compression_name.find("CIX_RocksDB_Experimental_v1;80;") != std::string::npos;
  if (!require(cix_sst, "persist CIX compression identity")) return 1;
  delete db;
  // The persisted custom-codec identity requires the compatible manager on
  // reopen; a fresh ordinary RocksDB configuration must refuse the SST.
  rocksdb::Options no_manager; no_manager.create_if_missing = false;
  no_manager.compression = rocksdb::kNoCompression;
  rocksdb::BlockBasedTableOptions no_manager_table; no_manager_table.format_version = 7;
  no_manager.table_factory.reset(rocksdb::NewBlockBasedTableFactory(no_manager_table));
  rocksdb::DB* no_manager_db = nullptr;
  const auto no_manager_open = rocksdb::DB::Open(no_manager, path, &no_manager_db);
  if (no_manager_open.ok()) {
    if (!require(no_manager_db != nullptr, "open without manager returns database")) return 1;
    // Some host configurations defer this compatibility check until block
    // decode. It must still reject the custom payload without a manager.
    std::string unreadable;
    if (!require(!no_manager_db->Get(rocksdb::ReadOptions(), "repetitive", &unreadable).ok(), "reject CIX block without manager")) return 1;
    delete no_manager_db;
  } else if (!require(no_manager_db == nullptr, "failed open leaves no database")) {
    return 1;
  }
  options.create_if_missing = false; db = nullptr;
  const auto reopen = rocksdb::DB::Open(options, path, &db);
  if (!require(reopen.ok() && db != nullptr, "reopen CIX database")) return 1;
  std::string value;
  if (!require(db->Get(rocksdb::ReadOptions(), "repetitive", &value).ok() && value == repetitive, "read repetitive value")) return 1;
  if (!require(db->Get(rocksdb::ReadOptions(), "random", &value).ok() && value == random, "read random value")) return 1;
  delete db;
  rocksdb::CixCompressionManagerStatsV1 stats{};
  if (!require(rocksdb::GetCixCompressionManagerStatsV1(manager, &stats), "read manager statistics")) return 1;
  if (!require(stats.encode_attempts && stats.cix_blocks && stats.raw_fallbacks && stats.decode_attempts, "record CIX and raw codec paths")) return 1;
  auto decoder = manager->GetDecompressor();
  if (!require(decoder != nullptr, "get default decompressor")) return 1;
  rocksdb::Decompressor::Args bad;
  bad.compression_type = rocksdb::kCustomCompression80; bad.compressed_data = rocksdb::Slice("bad", 3);
  const auto bad_size = decoder->ExtractUncompressedSize(bad);
  if (!require(!bad_size.ok(), "reject malformed CIX length header")) return 1;
  // The length header is checked before RocksDB allocates an output buffer.
  std::array<char, 16> oversized = {'C', 'I', 'X', 'R', 1, 0, 0, 0};
  oversized[11] = 4;  // 64 MiB in little-endian header form, one byte over the cap below.
  auto limited = rocksdb::NewCixCompressionManagerV1((64U << 20) - 1);
  if (!require(limited != nullptr, "create bounded CIX compression manager")) return 1;
  auto limited_decoder = limited->GetDecompressor();
  if (!require(limited_decoder != nullptr, "get bounded decompressor")) return 1;
  rocksdb::Decompressor::Args oversized_args;
  oversized_args.compression_type = rocksdb::kCustomCompression80;
  oversized_args.compressed_data = rocksdb::Slice(oversized.data(), oversized.size());
  const auto oversized_size = limited_decoder->ExtractUncompressedSize(oversized_args);
  if (!require(!oversized_size.ok(), "reject length above manager cap")) return 1;
  std::filesystem::remove_all(path);
}
