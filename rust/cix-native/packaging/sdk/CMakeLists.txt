cmake_minimum_required(VERSION 3.21)
option(CIX_SDK_LAYOUT_ONLY "Configure install layout without a compiler; not a platform qualification" OFF)
if(CIX_SDK_LAYOUT_ONLY)
    project(CIXSDK VERSION 0.1.0 LANGUAGES NONE)
    # A layout-only Windows fixture must not infer a host architecture.
    set(CMAKE_INSTALL_LIBDIR "lib" CACHE PATH "Layout-only library directory" FORCE)
else()
    project(CIXSDK VERSION 0.1.0 LANGUAGES C)
endif()

include(CMakePackageConfigHelpers)
include(GNUInstallDirs)

set(CIX_NATIVE_LIBRARY "" CACHE FILEPATH
    "Path to an already-built libcix_native shared or static library")
set(CIX_NATIVE_STATIC_LIBRARY "" CACHE FILEPATH
    "Optional path to the matching already-built libcix_native static archive")
set(CIX_NATIVE_IMPORT_LIBRARY "" CACHE FILEPATH
    "Windows only: matching selected libcix_native DLL import library")
if(NOT EXISTS "${CIX_NATIVE_LIBRARY}" OR IS_DIRECTORY "${CIX_NATIVE_LIBRARY}")
    message(FATAL_ERROR "Set CIX_NATIVE_LIBRARY to a selected built libcix_native shared library, Windows DLL, or Unix static archive")
endif()
if(CIX_NATIVE_STATIC_LIBRARY AND
   (NOT EXISTS "${CIX_NATIVE_STATIC_LIBRARY}" OR IS_DIRECTORY "${CIX_NATIVE_STATIC_LIBRARY}"))
    message(FATAL_ERROR "CIX_NATIVE_STATIC_LIBRARY must be an existing static archive when supplied")
endif()

get_filename_component(CIX_SOURCE_ROOT "${CMAKE_CURRENT_LIST_DIR}/../.." REALPATH)
get_filename_component(CIX_NATIVE_LIBRARY_NAME "${CIX_NATIVE_LIBRARY}" NAME)
set(CIX_NATIVE_IMPORT_LIBRARY_NAME "")
if(WIN32)
    if(NOT CIX_NATIVE_LIBRARY_NAME MATCHES "^cix_native\\.dll$")
        message(FATAL_ERROR "CIX_NATIVE_LIBRARY must be the selected cix_native.dll on Windows")
    endif()
    if(NOT CIX_NATIVE_IMPORT_LIBRARY OR
       NOT EXISTS "${CIX_NATIVE_IMPORT_LIBRARY}" OR
       IS_DIRECTORY "${CIX_NATIVE_IMPORT_LIBRARY}")
        message(FATAL_ERROR "Windows SDK installation requires matching CIX_NATIVE_IMPORT_LIBRARY")
    endif()
    get_filename_component(CIX_NATIVE_IMPORT_LIBRARY_NAME "${CIX_NATIVE_IMPORT_LIBRARY}" NAME)
    if(NOT CIX_NATIVE_IMPORT_LIBRARY_NAME MATCHES "^cix_native(\\.dll)?\\.lib$")
        message(FATAL_ERROR "CIX_NATIVE_IMPORT_LIBRARY must be cix_native.lib or cix_native.dll.lib")
    endif()
    if(CIX_NATIVE_STATIC_LIBRARY)
        message(FATAL_ERROR "CIX_NATIVE_STATIC_LIBRARY is not supported by the unqualified Windows SDK layout")
    endif()
elseif(NOT CIX_NATIVE_LIBRARY_NAME MATCHES "^libcix_native\\.(so|dylib|a)$")
    message(FATAL_ERROR "CIX_NATIVE_LIBRARY supports libcix_native.so, libcix_native.dylib, or libcix_native.a on this platform")
endif()
if(CIX_NATIVE_STATIC_LIBRARY)
    get_filename_component(CIX_NATIVE_STATIC_LIBRARY_NAME "${CIX_NATIVE_STATIC_LIBRARY}" NAME)
    if(NOT CIX_NATIVE_STATIC_LIBRARY_NAME STREQUAL "libcix_native.a")
        message(FATAL_ERROR "CIX_NATIVE_STATIC_LIBRARY must be named libcix_native.a")
    endif()
endif()

if(NOT IS_ABSOLUTE "${CMAKE_INSTALL_PREFIX}")
    message(FATAL_ERROR "CMAKE_INSTALL_PREFIX must be absolute for relocatable pkg-config metadata")
endif()
if(IS_ABSOLUTE "${CMAKE_INSTALL_LIBDIR}" OR IS_ABSOLUTE "${CMAKE_INSTALL_INCLUDEDIR}")
    message(FATAL_ERROR "CMAKE_INSTALL_LIBDIR and CMAKE_INSTALL_INCLUDEDIR must be prefix-relative")
endif()
set(CIX_PC_INSTALL_DIRECTORY "${CMAKE_INSTALL_PREFIX}/${CMAKE_INSTALL_LIBDIR}/pkgconfig")
file(RELATIVE_PATH CIX_PC_DIRECTORY_WITHIN_PREFIX
    "${CMAKE_INSTALL_PREFIX}" "${CIX_PC_INSTALL_DIRECTORY}")
if(CIX_PC_DIRECTORY_WITHIN_PREFIX MATCHES "^\\.\\.")
    message(FATAL_ERROR "CMAKE_INSTALL_LIBDIR must remain beneath CMAKE_INSTALL_PREFIX")
endif()
file(RELATIVE_PATH CIX_PC_PREFIX_FROM_PCDIR
    "${CIX_PC_INSTALL_DIRECTORY}" "${CMAKE_INSTALL_PREFIX}")

set(CIX_SDK_CONFIG_DIR "${CMAKE_INSTALL_LIBDIR}/cmake/CIX")
configure_package_config_file(
    "${CMAKE_CURRENT_LIST_DIR}/CIXConfig.cmake.in"
    "${CMAKE_CURRENT_BINARY_DIR}/CIXConfig.cmake"
    INSTALL_DESTINATION "${CIX_SDK_CONFIG_DIR}"
    PATH_VARS CMAKE_INSTALL_INCLUDEDIR CMAKE_INSTALL_LIBDIR)
write_basic_package_version_file(
    "${CMAKE_CURRENT_BINARY_DIR}/CIXConfigVersion.cmake"
    VERSION "${PROJECT_VERSION}"
    COMPATIBILITY SameMajorVersion)
configure_file("${CMAKE_CURRENT_LIST_DIR}/cix.pc.in"
               "${CMAKE_CURRENT_BINARY_DIR}/cix-native.pc" @ONLY)

install(FILES
    "${CIX_SOURCE_ROOT}/include/cix.h"
    "${CIX_SOURCE_ROOT}/include/cix.hpp"
    "${CIX_SOURCE_ROOT}/include/cix_stream.h"
    "${CIX_SOURCE_ROOT}/include/cix_formats.h"
    "${CIX_SOURCE_ROOT}/include/cix_dictionary.h"
    DESTINATION "${CMAKE_INSTALL_INCLUDEDIR}")
if(WIN32)
    # Keep both halves of the shared-library contract. CMake classifies the
    # DLL as a RUNTIME artifact and its import library as an ARCHIVE artifact.
    add_library(cix_native_prebuilt SHARED IMPORTED GLOBAL)
    set_target_properties(cix_native_prebuilt PROPERTIES
        IMPORTED_LOCATION "${CIX_NATIVE_LIBRARY}"
        IMPORTED_IMPLIB "${CIX_NATIVE_IMPORT_LIBRARY}")
    # CMake's IMPORTED_RUNTIME_ARTIFACTS deliberately excludes DLL import
    # libraries, so install the paired .lib explicitly in the conventional
    # ARCHIVE directory after staging the DLL runtime artifact.
    install(IMPORTED_RUNTIME_ARTIFACTS cix_native_prebuilt
        RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")
    install(FILES "${CIX_NATIVE_IMPORT_LIBRARY}"
        DESTINATION "${CMAKE_INSTALL_LIBDIR}")
else()
    install(FILES "${CIX_NATIVE_LIBRARY}" DESTINATION "${CMAKE_INSTALL_LIBDIR}")
    if(CIX_NATIVE_STATIC_LIBRARY)
        install(FILES "${CIX_NATIVE_STATIC_LIBRARY}" DESTINATION "${CMAKE_INSTALL_LIBDIR}")
    endif()
endif()
install(FILES
    "${CMAKE_CURRENT_BINARY_DIR}/CIXConfig.cmake"
    "${CMAKE_CURRENT_BINARY_DIR}/CIXConfigVersion.cmake"
    DESTINATION "${CIX_SDK_CONFIG_DIR}")
install(FILES "${CMAKE_CURRENT_BINARY_DIR}/cix-native.pc"
    DESTINATION "${CMAKE_INSTALL_LIBDIR}/pkgconfig")
install(FILES "${CMAKE_CURRENT_LIST_DIR}/README.md"
    DESTINATION "${CMAKE_INSTALL_DOCDIR}")
install(FILES "${CIX_SOURCE_ROOT}/examples/dictionary.c"
    DESTINATION "${CMAKE_INSTALL_DOCDIR}/examples")
