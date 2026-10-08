cmake_minimum_required(VERSION 3.16)
project(cix_squashfs_profile C)

add_library(cix_squashfs_profile STATIC src/cix_squashfs_profile.c)
target_compile_features(cix_squashfs_profile PUBLIC c_std_11)
target_include_directories(cix_squashfs_profile PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/include)

# This target intentionally has no SquashFS-tools dependency.  The contract
# harness is source-only until a pinned compatible tools integration is added.
add_executable(cix_squashfs_profile_contract tests/profile_contract.c)
target_link_libraries(cix_squashfs_profile_contract PRIVATE cix_squashfs_profile)
