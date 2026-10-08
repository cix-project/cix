// SPDX-License-Identifier: MIT
#include <cix.hpp>

#include <utility>
#include <cstdint>
#include <vector>

int main() {
    const std::vector<std::uint8_t> source{
        'C', 'I', 'X', ' ', 'C', '+', '+', ' ', 'S', 'D', 'K'};
    cix::Context context;
    const std::vector<std::uint8_t> archive = context.encode(source);
    cix::Context moved = std::move(context);
    const std::vector<std::uint8_t> restored = moved.decode(archive);
    const std::vector<std::uint8_t> empty_archive = moved.encode({});
    const std::vector<std::uint8_t> empty_restored = moved.decode(empty_archive);
    if (restored != source || !empty_restored.empty()) {
        return 1;
    }

    cix_options_v1 invalid = cix::Context::default_options();
    invalid.memory_limit = 0;
    try {
        cix::Context rejected(invalid);
        return 2;
    } catch (const cix::StatusError &error) {
        return error.status() == CIX_STATUS_INVALID_OPTIONS ? 0 : 3;
    }
}
