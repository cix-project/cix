cmake_minimum_required(VERSION 3.20)
project(cix_jxl_bridge LANGUAGES CXX)

# This bridge intentionally accepts only an explicitly provisioned libjxl
# prefix. It never falls back to the host's unrelated runtime library.
set(CIX_JXL_ROOT "" CACHE PATH "Installed libjxl 0.12 prefix")
if(NOT IS_DIRECTORY "${CIX_JXL_ROOT}")
    message(FATAL_ERROR "Set CIX_JXL_ROOT to the provisioned libjxl 0.12 install prefix")
endif()

find_path(CIX_JXL_INCLUDE_DIR jxl/encode.h
    HINTS "${CIX_JXL_ROOT}/include" "${CIX_JXL_ROOT}/lib/include"
    NO_DEFAULT_PATH REQUIRED)
find_library(CIX_JXL_LIBRARY NAMES jxl
    HINTS "${CIX_JXL_ROOT}/lib" "${CIX_JXL_ROOT}/lib64"
    NO_DEFAULT_PATH REQUIRED)
find_library(CIX_JXL_THREADS_LIBRARY NAMES jxl_threads
    HINTS "${CIX_JXL_ROOT}/lib" "${CIX_JXL_ROOT}/lib64"
    NO_DEFAULT_PATH REQUIRED)
find_library(CIX_LZMA_LIBRARY NAMES lzma REQUIRED)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

add_library(cix_jxl_bridge SHARED cix_jxl_bridge.cpp)
set_target_properties(cix_jxl_bridge PROPERTIES
    CXX_VISIBILITY_PRESET hidden
    VISIBILITY_INLINES_HIDDEN YES
    OUTPUT_NAME cix_jxl_bridge)
target_compile_definitions(cix_jxl_bridge PRIVATE CIX_JXL_BRIDGE_BUILDING)
target_include_directories(cix_jxl_bridge PRIVATE "${CIX_JXL_INCLUDE_DIR}")
target_link_libraries(cix_jxl_bridge PRIVATE
    "${CIX_JXL_LIBRARY}"
    "${CIX_JXL_THREADS_LIBRARY}"
    "${CIX_LZMA_LIBRARY}")
if(MSVC)
    target_compile_options(cix_jxl_bridge PRIVATE /O2 /fp:strict)
else()
    target_compile_options(cix_jxl_bridge PRIVATE
        -O3 -fno-fast-math -ffp-contract=off -fno-strict-aliasing)
endif()

# The package recipe must point the bridge at its packaged libjxl sibling,
# rather than resolving a host libjxl at run time.
if(UNIX AND NOT APPLE)
    set_target_properties(cix_jxl_bridge PROPERTIES INSTALL_RPATH "$ORIGIN")
elseif(APPLE)
    set_target_properties(cix_jxl_bridge PROPERTIES INSTALL_RPATH "@loader_path")
endif()

include(GNUInstallDirs)
install(TARGETS cix_jxl_bridge
    LIBRARY DESTINATION "${CMAKE_INSTALL_LIBDIR}"
    RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}"
    ARCHIVE DESTINATION "${CMAKE_INSTALL_LIBDIR}")
install(FILES cix_jxl_bridge.h DESTINATION "${CMAKE_INSTALL_INCLUDEDIR}/cix")
