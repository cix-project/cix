cmake_minimum_required(VERSION 3.21)
project(cix_7zip_experimental_coder LANGUAGES CXX)

# This is an explicit installed-SDK consumer.  It neither downloads 7-Zip nor
# searches the workstation for an arbitrary 7-Zip or CIX installation.
set(CIX_NATIVE_INCLUDE_DIR "" CACHE PATH
    "Absolute installed directory containing cix.h and cix_stream.h")
set(CIX_NATIVE_LIBRARY "" CACHE FILEPATH
    "Absolute path to the selected installed libcix_native library")
set(CIX_7ZIP_SDK_ROOT "" CACHE PATH
    "Absolute root of the pinned official 7-Zip SDK source tree")
set(CIX_7ZIP_SDK_VERSION "26.03" CACHE STRING
    "Pinned official 7-Zip SDK version accepted by this adapter")
set(CIX_7ZIP_SDK_ARCHIVE_SHA256 "" CACHE STRING
    "Recorded SHA-256 of the exact official SDK archive used for this build")

foreach(required IN ITEMS CIX_NATIVE_LIBRARY)
  if(NOT IS_ABSOLUTE "${${required}}" OR NOT EXISTS "${${required}}")
    message(FATAL_ERROR "${required} must be an existing absolute selected library")
  endif()
endforeach()
if(NOT IS_ABSOLUTE "${CIX_NATIVE_INCLUDE_DIR}" OR
   NOT EXISTS "${CIX_NATIVE_INCLUDE_DIR}/cix.h" OR
   NOT EXISTS "${CIX_NATIVE_INCLUDE_DIR}/cix_stream.h")
  message(FATAL_ERROR "CIX_NATIVE_INCLUDE_DIR must be an absolute installed directory containing cix.h and cix_stream.h")
endif()
if(NOT IS_ABSOLUTE "${CIX_7ZIP_SDK_ROOT}" OR
   NOT EXISTS "${CIX_7ZIP_SDK_ROOT}/CPP/7zip/ICoder.h" OR
   NOT EXISTS "${CIX_7ZIP_SDK_ROOT}/CPP/7zip/IStream.h" OR
   NOT EXISTS "${CIX_7ZIP_SDK_ROOT}/CPP/7zip/Common/RegisterCodec.h" OR
   NOT EXISTS "${CIX_7ZIP_SDK_ROOT}/CPP/7zip/Compress/CodecExports.cpp" OR
   NOT EXISTS "${CIX_7ZIP_SDK_ROOT}/CPP/Common/MyCom.h" OR
   NOT EXISTS "${CIX_7ZIP_SDK_ROOT}/CPP/Common/MyWindows.cpp")
  message(FATAL_ERROR "CIX_7ZIP_SDK_ROOT must be the absolute official SDK root with CPP/7zip and CPP/Common headers")
endif()
if(NOT CIX_7ZIP_SDK_VERSION STREQUAL "26.03")
  message(FATAL_ERROR "This adapter is source-pinned to official 7-Zip SDK 26.03")
endif()
string(LENGTH "${CIX_7ZIP_SDK_ARCHIVE_SHA256}" CIX_7ZIP_SHA_LENGTH)
if(NOT CIX_7ZIP_SHA_LENGTH EQUAL 64 OR
   NOT CIX_7ZIP_SDK_ARCHIVE_SHA256 MATCHES "^[0-9A-Fa-f]+$")
  message(FATAL_ERROR "Record the exact official SDK archive SHA-256 in CIX_7ZIP_SDK_ARCHIVE_SHA256")
endif()

add_library(cix_7zip_experimental_coder STATIC cix_7zip_coder.cpp)
target_compile_features(cix_7zip_experimental_coder PUBLIC cxx_std_17)
target_include_directories(cix_7zip_experimental_coder
  PUBLIC
    "$<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}>"
    "$<BUILD_INTERFACE:${CIX_7ZIP_SDK_ROOT}/CPP>"
  PRIVATE
    "${CIX_NATIVE_INCLUDE_DIR}")
target_link_libraries(cix_7zip_experimental_coder PUBLIC "${CIX_NATIVE_LIBRARY}")
set(CIX_7ZIP_DEFAULT_INPUT_LIMIT "67108864" CACHE STRING
    "Deployment-local maximum input bytes admitted by one CIX 7-Zip coder")
set(CIX_7ZIP_DEFAULT_OUTPUT_LIMIT "134217728" CACHE STRING
    "Deployment-local maximum output bytes admitted by one CIX 7-Zip decoder")
set(CIX_7ZIP_DEFAULT_MEMORY_LIMIT "536870912" CACHE STRING
    "Deployment-local CIX memory admission limit in bytes")
set(CIX_7ZIP_DEFAULT_WORKERS "1" CACHE STRING
    "Deployment-local CIX worker count")
foreach(limit IN ITEMS CIX_7ZIP_DEFAULT_INPUT_LIMIT CIX_7ZIP_DEFAULT_OUTPUT_LIMIT
    CIX_7ZIP_DEFAULT_MEMORY_LIMIT CIX_7ZIP_DEFAULT_WORKERS)
  if(NOT "${${limit}}" MATCHES "^[1-9][0-9]*$")
    message(FATAL_ERROR "${limit} must be a positive decimal deployment-local limit")
  endif()
endforeach()
set(CIX_7ZIP_CODER_DEFINITIONS
  CIX_7ZIP_SDK_VERSION_STRING="${CIX_7ZIP_SDK_VERSION}"
  CIX_7ZIP_DEFAULT_INPUT_LIMIT=${CIX_7ZIP_DEFAULT_INPUT_LIMIT}
  CIX_7ZIP_DEFAULT_OUTPUT_LIMIT=${CIX_7ZIP_DEFAULT_OUTPUT_LIMIT}
  CIX_7ZIP_DEFAULT_MEMORY_LIMIT=${CIX_7ZIP_DEFAULT_MEMORY_LIMIT}
  CIX_7ZIP_DEFAULT_WORKERS=${CIX_7ZIP_DEFAULT_WORKERS})
target_compile_definitions(cix_7zip_experimental_coder PRIVATE ${CIX_7ZIP_CODER_DEFINITIONS})

# The module compiles the official CodecExports.cpp unchanged and supplies the
# documented g_Codecs/g_NumCodecs registry globals in CIX-owned source.
add_library(cix_7zip_experimental_module SHARED
  cix_7zip_module.cpp cix_7zip_coder.cpp
  "${CIX_7ZIP_SDK_ROOT}/CPP/7zip/Compress/CodecExports.cpp"
  "${CIX_7ZIP_SDK_ROOT}/CPP/Common/MyWindows.cpp")
target_compile_features(cix_7zip_experimental_module PRIVATE cxx_std_17)
target_include_directories(cix_7zip_experimental_module PRIVATE
  "${CMAKE_CURRENT_SOURCE_DIR}" "${CIX_NATIVE_INCLUDE_DIR}"
  "${CIX_7ZIP_SDK_ROOT}/CPP")
target_link_libraries(cix_7zip_experimental_module PRIVATE "${CIX_NATIVE_LIBRARY}")
target_compile_definitions(cix_7zip_experimental_module PRIVATE ${CIX_7ZIP_CODER_DEFINITIONS})
set_target_properties(cix_7zip_experimental_module PROPERTIES
  OUTPUT_NAME cix_7zip_experimental_module
  INSTALL_RPATH_USE_LINK_PATH FALSE)

# The selected CIX DSO is a companion of this external codec, never a PATH or
# host-global fallback. The host discovers modules in its sibling Codecs/
# directory, so the module and selected library install there together.
if(APPLE)
  set_target_properties(cix_7zip_experimental_module PROPERTIES
    INSTALL_RPATH "@loader_path")
elseif(UNIX)
  set_target_properties(cix_7zip_experimental_module PROPERTIES
    INSTALL_RPATH "$ORIGIN")
endif()
install(TARGETS cix_7zip_experimental_module
  LIBRARY DESTINATION Codecs
  RUNTIME DESTINATION Codecs)
install(FILES "${CIX_NATIVE_LIBRARY}" DESTINATION Codecs)

option(CIX_7ZIP_BUILD_CONTRACT "Build the CIX-owned 7-Zip module contract" OFF)
if(CIX_7ZIP_BUILD_CONTRACT)
  add_executable(cix_7zip_contract cix_7zip_contract.cpp)
  target_compile_features(cix_7zip_contract PRIVATE cxx_std_17)
  target_include_directories(cix_7zip_contract PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}" "${CIX_7ZIP_SDK_ROOT}/CPP")
  target_link_libraries(cix_7zip_contract PRIVATE cix_7zip_experimental_module)
endif()

# This creates an explicit CIX-owned Codecs/ payload. It does not install into
# a stock 7-Zip directory or assert host discovery/format compatibility.
