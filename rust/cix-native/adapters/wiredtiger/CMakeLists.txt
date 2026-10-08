cmake_minimum_required(VERSION 3.21)
project(cix_wiredtiger_compressor LANGUAGES C)

# This is an installed-SDK consumer. It never fetches WiredTiger or resolves
# an unrelated CIX/WiredTiger runtime from the host.
set(CIX_NATIVE_INCLUDE_DIR "" CACHE PATH
    "Absolute installed directory containing cix.h")
set(CIX_NATIVE_LIBRARY "" CACHE FILEPATH
    "Absolute path to a qualified libcix_native shared library")
set(WIREDTIGER_INCLUDE_DIR "" CACHE PATH
    "Absolute installed directory containing wiredtiger.h")
set(WIREDTIGER_LIBRARY "" CACHE FILEPATH
    "Absolute path to the pinned libwiredtiger shared library")
set(CIX_WIREDTIGER_MAX_BLOCK_BYTES "67108864" CACHE STRING
    "Maximum WiredTiger block accepted by the CIX compressor")
set(CIX_WIREDTIGER_PINNED_MAJOR "11" CACHE STRING "Pinned WiredTiger major version")
set(CIX_WIREDTIGER_PINNED_MINOR "3" CACHE STRING "Pinned WiredTiger minor version")
set(CIX_WIREDTIGER_PINNED_PATCH "1" CACHE STRING "Pinned WiredTiger patch version")

foreach(required_path IN ITEMS CIX_NATIVE_LIBRARY WIREDTIGER_LIBRARY)
    if(NOT IS_ABSOLUTE "${${required_path}}" OR NOT EXISTS "${${required_path}}")
        message(FATAL_ERROR "${required_path} must name an existing absolute installed library")
    endif()
endforeach()
if(NOT IS_ABSOLUTE "${CIX_NATIVE_INCLUDE_DIR}" OR
        NOT EXISTS "${CIX_NATIVE_INCLUDE_DIR}/cix.h")
    message(FATAL_ERROR "CIX_NATIVE_INCLUDE_DIR must be an absolute installed directory containing cix.h")
endif()
if(NOT IS_ABSOLUTE "${WIREDTIGER_INCLUDE_DIR}" OR
        NOT EXISTS "${WIREDTIGER_INCLUDE_DIR}/wiredtiger.h")
    message(FATAL_ERROR "WIREDTIGER_INCLUDE_DIR must be an absolute installed directory containing wiredtiger.h")
endif()
if(NOT CIX_WIREDTIGER_MAX_BLOCK_BYTES MATCHES "^[0-9]+$" OR
        CIX_WIREDTIGER_MAX_BLOCK_BYTES LESS 1 OR
        CIX_WIREDTIGER_MAX_BLOCK_BYTES GREATER 134217728)
    message(FATAL_ERROR
        "CIX_WIREDTIGER_MAX_BLOCK_BYTES must be a positive bound no larger than CIX's 128 MiB native admission")
endif()

file(READ "${WIREDTIGER_INCLUDE_DIR}/wiredtiger.h" CIX_WT_HEADER)
foreach(component IN ITEMS MAJOR MINOR PATCH)
    string(REGEX MATCH "WIREDTIGER_VERSION_${component}[ \t]+${CIX_WIREDTIGER_PINNED_${component}}([^0-9]|$)"
        _cix_wt_version_match "${CIX_WT_HEADER}")
    if(_cix_wt_version_match STREQUAL "")
        message(FATAL_ERROR
            "wiredtiger.h does not match pinned ${CIX_WIREDTIGER_PINNED_MAJOR}.${CIX_WIREDTIGER_PINNED_MINOR}.${CIX_WIREDTIGER_PINNED_PATCH}")
    endif()
endforeach()
string(FIND "${CIX_WT_HEADER}" "struct __wt_compressor" _cix_wt_compressor_api)
if(_cix_wt_compressor_api EQUAL -1)
    message(FATAL_ERROR "The supplied WiredTiger header lacks WT_COMPRESSOR")
endif()

add_library(cix_wt_compressor MODULE src/cix_wt_compressor.c)
target_compile_features(cix_wt_compressor PRIVATE c_std_11)
target_include_directories(cix_wt_compressor PRIVATE
    "${CIX_NATIVE_INCLUDE_DIR}" "${WIREDTIGER_INCLUDE_DIR}")
target_link_libraries(cix_wt_compressor PRIVATE
    "${CIX_NATIVE_LIBRARY}" "${WIREDTIGER_LIBRARY}")
target_compile_definitions(cix_wt_compressor PRIVATE
    CIX_WIREDTIGER_MAX_BLOCK_BYTES=${CIX_WIREDTIGER_MAX_BLOCK_BYTES})
set_target_properties(cix_wt_compressor PROPERTIES
    PREFIX ""
    OUTPUT_NAME "cix_wt_compressor"
    C_VISIBILITY_PRESET hidden
    VISIBILITY_INLINES_HIDDEN YES)

if(UNIX AND NOT APPLE)
    set_target_properties(cix_wt_compressor PROPERTIES INSTALL_RPATH "$ORIGIN/../..")
elseif(APPLE)
    set_target_properties(cix_wt_compressor PROPERTIES INSTALL_RPATH "@loader_path/../..")
endif()

include(GNUInstallDirs)
install(TARGETS cix_wt_compressor
    LIBRARY DESTINATION "${CMAKE_INSTALL_LIBDIR}/wiredtiger/ext"
    RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")

option(CIX_WIREDTIGER_BUILD_CONTRACT_TEST
    "Build the installed-host WiredTiger extension contract test" OFF)
if(CIX_WIREDTIGER_BUILD_CONTRACT_TEST)
    enable_testing()
    # This symbol exists only in the contract build and exposes the actual
    # callback table for boundary calls that cannot be reached through a
    # database page without relying on internal WiredTiger implementation.
    add_library(cix_wt_compressor_contract OBJECT src/cix_wt_compressor.c)
    target_compile_features(cix_wt_compressor_contract PRIVATE c_std_11)
    target_include_directories(cix_wt_compressor_contract PRIVATE
        "${CIX_NATIVE_INCLUDE_DIR}" "${WIREDTIGER_INCLUDE_DIR}")
    target_compile_definitions(cix_wt_compressor_contract PRIVATE
        CIX_WIREDTIGER_MAX_BLOCK_BYTES=${CIX_WIREDTIGER_MAX_BLOCK_BYTES}
        CIX_WT_CONTRACT_TESTING)
    add_executable(cix_wt_contract tests/wiredtiger_contract.c)
    target_sources(cix_wt_contract PRIVATE $<TARGET_OBJECTS:cix_wt_compressor_contract>)
    target_compile_features(cix_wt_contract PRIVATE c_std_11)
    target_include_directories(cix_wt_contract PRIVATE "${WIREDTIGER_INCLUDE_DIR}")
    target_link_libraries(cix_wt_contract PRIVATE
        "${CIX_NATIVE_LIBRARY}" "${WIREDTIGER_LIBRARY}")
    target_compile_definitions(cix_wt_contract PRIVATE CIX_WT_CONTRACT_TESTING)
    add_test(NAME cix_wt_contract
        COMMAND cix_wt_contract "$<TARGET_FILE:cix_wt_compressor>")
endif()
