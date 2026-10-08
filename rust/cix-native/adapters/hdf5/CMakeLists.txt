cmake_minimum_required(VERSION 3.21)
project(CIXHDF5Filter LANGUAGES C)

# This is an installed-SDK consumer.  It intentionally does not select a
# public HDF5 filter number: an experimental deployment must choose one.
set(CIX_NATIVE_LIBRARY "" CACHE FILEPATH
    "Absolute path to a qualified libcix_native shared library")
set(CIX_NATIVE_INCLUDE_DIR "" CACHE PATH
    "Absolute installed directory containing cix.h")
set(CIX_HDF5_FILTER_ID "" CACHE STRING
    "Explicit experimental HDF5 filter ID (256..65535)")
set(CIX_HDF5_MAX_CHUNK_MIB 64 CACHE STRING
    "Deployment maximum uncompressed HDF5 chunk size in MiB (1..128)")
set(CIX_HDF5_MAX_MEMORY_MIB 512 CACHE STRING
    "Deployment maximum native CIX working-memory request in MiB")
set(CIX_HDF5_MAX_WORKERS 4 CACHE STRING
    "Deployment maximum CIX worker request per HDF5 chunk")

if(NOT IS_ABSOLUTE "${CIX_NATIVE_LIBRARY}" OR NOT EXISTS "${CIX_NATIVE_LIBRARY}")
    message(FATAL_ERROR "CIX_NATIVE_LIBRARY must name an existing absolute installed SDK library")
endif()
if(NOT IS_ABSOLUTE "${CIX_NATIVE_INCLUDE_DIR}" OR NOT EXISTS "${CIX_NATIVE_INCLUDE_DIR}/cix.h")
    message(FATAL_ERROR "CIX_NATIVE_INCLUDE_DIR must be an absolute installed directory containing cix.h")
endif()
if(NOT CIX_HDF5_FILTER_ID MATCHES "^[0-9]+$" OR CIX_HDF5_FILTER_ID LESS 256 OR CIX_HDF5_FILTER_ID GREATER 65535)
    message(FATAL_ERROR "Set CIX_HDF5_FILTER_ID to an explicit experimental ID in HDF5's 256..65535 range")
endif()
foreach(CIX_HDF5_CAP CIX_HDF5_MAX_CHUNK_MIB CIX_HDF5_MAX_MEMORY_MIB CIX_HDF5_MAX_WORKERS)
    if(NOT ${CIX_HDF5_CAP} MATCHES "^[0-9]+$" OR ${CIX_HDF5_CAP} LESS 1 OR ${CIX_HDF5_CAP} GREATER 4294967295)
        message(FATAL_ERROR "${CIX_HDF5_CAP} must be a positive unsigned 32-bit integer")
    endif()
endforeach()
if(CIX_HDF5_MAX_CHUNK_MIB GREATER 128)
    message(FATAL_ERROR "CIX_HDF5_MAX_CHUNK_MIB cannot exceed the native CIX 128 MiB input admission")
endif()
if(CIX_HDF5_MAX_WORKERS GREATER 4096)
    message(FATAL_ERROR "CIX_HDF5_MAX_WORKERS cannot exceed 4096")
endif()

# H5PL dynamic filters appeared in 1.8.11.  This adapter uses the public
# H5allocate_memory/H5free_memory allocation API and is qualified from 1.10.
find_package(HDF5 1.10 REQUIRED COMPONENTS C)

add_library(H5Zcix MODULE src/cix_hdf5_filter.c)
target_include_directories(H5Zcix PRIVATE "${CIX_NATIVE_INCLUDE_DIR}" ${HDF5_INCLUDE_DIRS})
target_link_libraries(H5Zcix PRIVATE "${CIX_NATIVE_LIBRARY}" ${HDF5_LIBRARIES})
target_compile_definitions(H5Zcix PRIVATE
    CIX_HDF5_FILTER_ID=${CIX_HDF5_FILTER_ID}
    CIX_HDF5_MAX_CHUNK_MIB=${CIX_HDF5_MAX_CHUNK_MIB}
    CIX_HDF5_MAX_MEMORY_MIB=${CIX_HDF5_MAX_MEMORY_MIB}
    CIX_HDF5_MAX_WORKERS=${CIX_HDF5_MAX_WORKERS})
# H5PLget_plugin_type and H5PLget_plugin_info are the dynamic-loader ABI and
# must remain exported from the module.
# HDF5's POSIX loader scans lib*.so/lib*.dylib; keep the platform prefix.
set_target_properties(H5Zcix PROPERTIES OUTPUT_NAME "H5Zcix" C_STANDARD 99 C_STANDARD_REQUIRED YES)

option(CIX_HDF5_BUILD_CONTRACT_TEST "Build the installed-host HDF5 contract test" OFF)
if(CIX_HDF5_BUILD_CONTRACT_TEST)
    enable_testing()
    add_executable(cix_hdf5_filter_contract tests/filter_contract.c)
    target_include_directories(cix_hdf5_filter_contract PRIVATE ${HDF5_INCLUDE_DIRS})
    target_link_libraries(cix_hdf5_filter_contract PRIVATE ${HDF5_LIBRARIES})
    target_compile_definitions(cix_hdf5_filter_contract PRIVATE
        CIX_HDF5_FILTER_ID=${CIX_HDF5_FILTER_ID}
        CIX_HDF5_MAX_CHUNK_MIB=${CIX_HDF5_MAX_CHUNK_MIB}
        CIX_HDF5_MAX_MEMORY_MIB=${CIX_HDF5_MAX_MEMORY_MIB}
        CIX_HDF5_MAX_WORKERS=${CIX_HDF5_MAX_WORKERS})
    set_target_properties(cix_hdf5_filter_contract PROPERTIES C_STANDARD 99 C_STANDARD_REQUIRED YES)
    add_test(NAME cix_hdf5_filter_contract
        COMMAND cix_hdf5_filter_contract "${CMAKE_CURRENT_BINARY_DIR}/cix-filter-contract.h5")
    set_tests_properties(cix_hdf5_filter_contract PROPERTIES
        ENVIRONMENT "HDF5_PLUGIN_PATH=${CMAKE_CURRENT_BINARY_DIR}")
endif()
