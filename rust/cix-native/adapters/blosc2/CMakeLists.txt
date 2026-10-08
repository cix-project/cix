cmake_minimum_required(VERSION 3.21)
project(cix_blosc2_adapter LANGUAGES C)

# This project is an installed-SDK consumer.  It deliberately does not fetch
# Blosc2 or discover an arbitrary host CIX library.
set(CIX_NATIVE_INCLUDE_DIR "" CACHE PATH
    "Absolute installed directory containing cix.h")
set(CIX_NATIVE_LIBRARY "" CACHE FILEPATH
    "Absolute path to a qualified libcix_native shared library")
set(BLOSC2_INCLUDE_DIR "" CACHE PATH
    "Absolute installed directory containing blosc2.h")
set(BLOSC2_LIBRARY "" CACHE FILEPATH
    "Absolute path to the pinned libblosc2 shared library")
set(CIX_BLOSC2_EXPERIMENTAL_CODEC_ID "" CACHE STRING
    "Explicit Blosc2 user codec ID (160..255)")
set(CIX_BLOSC2_MAX_CHUNK_BYTES "67108864" CACHE STRING
    "Maximum uncompressed Blosc2 block offered to the CIX codec callback")
set(CIX_BLOSC2_PINNED_VERSION "3.3.2" CACHE STRING
    "Exact Blosc2 header version accepted by this adapter")

foreach(required_path IN ITEMS CIX_NATIVE_LIBRARY BLOSC2_LIBRARY)
    if(NOT IS_ABSOLUTE "${${required_path}}" OR NOT EXISTS "${${required_path}}")
        message(FATAL_ERROR "${required_path} must name an existing absolute installed library")
    endif()
endforeach()
if(NOT IS_ABSOLUTE "${CIX_NATIVE_INCLUDE_DIR}" OR
        NOT EXISTS "${CIX_NATIVE_INCLUDE_DIR}/cix.h")
    message(FATAL_ERROR "CIX_NATIVE_INCLUDE_DIR must be an absolute installed directory containing cix.h")
endif()
if(NOT IS_ABSOLUTE "${BLOSC2_INCLUDE_DIR}" OR
        NOT EXISTS "${BLOSC2_INCLUDE_DIR}/blosc2.h")
    message(FATAL_ERROR "BLOSC2_INCLUDE_DIR must be an absolute installed directory containing blosc2.h")
endif()
if(NOT CIX_BLOSC2_EXPERIMENTAL_CODEC_ID MATCHES "^[0-9]+$" OR
        CIX_BLOSC2_EXPERIMENTAL_CODEC_ID LESS 160 OR
        CIX_BLOSC2_EXPERIMENTAL_CODEC_ID GREATER 255)
    message(FATAL_ERROR
        "Set CIX_BLOSC2_EXPERIMENTAL_CODEC_ID to an explicit Blosc2 user codec ID in 160..255")
endif()
if(NOT CIX_BLOSC2_MAX_CHUNK_BYTES MATCHES "^[0-9]+$" OR
        CIX_BLOSC2_MAX_CHUNK_BYTES LESS 1 OR
        CIX_BLOSC2_MAX_CHUNK_BYTES GREATER 134217728)
    message(FATAL_ERROR
        "CIX_BLOSC2_MAX_CHUNK_BYTES must be an explicit positive bound no larger than CIX's 128 MiB native admission")
endif()

file(READ "${BLOSC2_INCLUDE_DIR}/blosc2.h" CIX_BLOSC2_HEADER)
string(REGEX MATCH "BLOSC2_VERSION_STRING[ \t]+\\\"([^\\\"]+)\\\""
    _cix_blosc2_version_match "${CIX_BLOSC2_HEADER}")
if(NOT "${CMAKE_MATCH_1}" STREQUAL "${CIX_BLOSC2_PINNED_VERSION}")
    message(FATAL_ERROR
        "blosc2.h version must be ${CIX_BLOSC2_PINNED_VERSION}; provision the pinned upstream SDK")
endif()
string(FIND "${CIX_BLOSC2_HEADER}" "blosc2_register_codec" _cix_blosc2_registration_api)
if(_cix_blosc2_registration_api EQUAL -1)
    message(FATAL_ERROR "The supplied Blosc2 header lacks blosc2_register_codec")
endif()

add_library(cix_blosc2_adapter SHARED src/cix_blosc2_adapter.c)
target_compile_features(cix_blosc2_adapter PRIVATE c_std_11)
target_include_directories(cix_blosc2_adapter PRIVATE
    "${CIX_NATIVE_INCLUDE_DIR}" "${BLOSC2_INCLUDE_DIR}" "${CMAKE_CURRENT_SOURCE_DIR}/include")
target_link_libraries(cix_blosc2_adapter PRIVATE
    "${CIX_NATIVE_LIBRARY}" "${BLOSC2_LIBRARY}")
target_compile_definitions(cix_blosc2_adapter PRIVATE
    CIX_BLOSC2_BUILDING=1
    CIX_BLOSC2_EXPERIMENTAL_CODEC_ID=${CIX_BLOSC2_EXPERIMENTAL_CODEC_ID}
    CIX_BLOSC2_MAX_CHUNK_BYTES=${CIX_BLOSC2_MAX_CHUNK_BYTES})
set_target_properties(cix_blosc2_adapter PROPERTIES
    C_VISIBILITY_PRESET hidden
    VISIBILITY_INLINES_HIDDEN YES
    OUTPUT_NAME cix_blosc2_adapter)

if(UNIX AND NOT APPLE)
    set_target_properties(cix_blosc2_adapter PROPERTIES INSTALL_RPATH "$ORIGIN")
elseif(APPLE)
    set_target_properties(cix_blosc2_adapter PROPERTIES INSTALL_RPATH "@loader_path")
endif()

include(GNUInstallDirs)
install(TARGETS cix_blosc2_adapter
    LIBRARY DESTINATION "${CMAKE_INSTALL_LIBDIR}"
    RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")
install(FILES include/cix_blosc2_adapter.h
    DESTINATION "${CMAKE_INSTALL_INCLUDEDIR}/cix")

option(CIX_BLOSC2_BUILD_CONTRACT_TEST
    "Build the installed-host CIX/Blosc2 contract test" OFF)
if(CIX_BLOSC2_BUILD_CONTRACT_TEST)
    enable_testing()
    add_executable(cix_blosc2_contract tests/blosc2_cix_contract.c)
    target_compile_features(cix_blosc2_contract PRIVATE c_std_11)
    target_include_directories(cix_blosc2_contract PRIVATE
        "${BLOSC2_INCLUDE_DIR}" "${CMAKE_CURRENT_SOURCE_DIR}/include")
    target_link_libraries(cix_blosc2_contract PRIVATE
        cix_blosc2_adapter "${BLOSC2_LIBRARY}")
    target_compile_definitions(cix_blosc2_contract PRIVATE
        CIX_BLOSC2_TEST_CODEC_ID=${CIX_BLOSC2_EXPERIMENTAL_CODEC_ID})
    add_test(NAME cix_blosc2_contract COMMAND cix_blosc2_contract)
endif()
