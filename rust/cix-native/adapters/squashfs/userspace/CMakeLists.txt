cmake_minimum_required(VERSION 3.16)
project(cix_squashfs_userspace C)
add_library(cix_sqfs_userspace STATIC cix_sqfs_userspace.c)
target_compile_features(cix_sqfs_userspace PUBLIC c_std_11)
target_include_directories(cix_sqfs_userspace PUBLIC include ../profile/include)

# Source-specific overlay targets are intentionally not enabled here.  They
# require the pinned third-party checkout stated in README.md.
