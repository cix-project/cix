cmake_minimum_required(VERSION 3.20)
project(cix_spatial_bridge LANGUAGES CXX)

# Inputs are selected by the distribution build. CIX_SPATIAL_ROOT is retained
# as the one-root compatibility spelling; CIX_SPATIAL_DEPENDENCY_ROOTS permits
# separate target SDK prefixes. Neither setting permits host discovery.
set(CIX_SPATIAL_ROOT "" CACHE PATH "Provisioned CharLS/OpenJPEG prefix")
set(CIX_SPATIAL_DEPENDENCY_ROOTS "" CACHE STRING
    "Semicolon-separated immutable CharLS/OpenJPEG target SDK prefixes")
set(CIX_CHARLS_LIBRARY "" CACHE FILEPATH "Selected CharLS library in the provisioned prefix")
set(CIX_OPENJPEG_LIBRARY "" CACHE FILEPATH "Selected OpenJPEG library in the provisioned prefix")
set(CIX_SPATIAL_ROOTS ${CIX_SPATIAL_DEPENDENCY_ROOTS})
if(CIX_SPATIAL_ROOT)
    list(PREPEND CIX_SPATIAL_ROOTS "${CIX_SPATIAL_ROOT}")
endif()
list(REMOVE_DUPLICATES CIX_SPATIAL_ROOTS)
if(NOT CIX_SPATIAL_ROOTS)
    message(FATAL_ERROR "Set CIX_SPATIAL_ROOT or CIX_SPATIAL_DEPENDENCY_ROOTS to immutable target SDK prefixes")
endif()

set(CIX_SPATIAL_INCLUDE_HINTS)
set(CIX_SPATIAL_LIBRARY_HINTS)
foreach(root IN LISTS CIX_SPATIAL_ROOTS)
    if(NOT IS_DIRECTORY "${root}")
        message(FATAL_ERROR "CIX spatial dependency root is not a directory: ${root}")
    endif()
    list(APPEND CIX_SPATIAL_INCLUDE_HINTS "${root}/include" "${root}/usr/include")
    # CMAKE_LIBRARY_ARCHITECTURE comes from the configured target toolchain;
    # it is not a hard-coded or host-derived architecture string.
    list(APPEND CIX_SPATIAL_LIBRARY_HINTS
        "${root}/lib"
        "${root}/lib64"
        "${root}/usr/lib"
        "${root}/usr/lib/${CMAKE_LIBRARY_ARCHITECTURE}")
endforeach()
list(REMOVE_DUPLICATES CIX_SPATIAL_INCLUDE_HINTS)
list(REMOVE_DUPLICATES CIX_SPATIAL_LIBRARY_HINTS)

find_path(CIX_CHARLS_INCLUDE charls/charls.h HINTS ${CIX_SPATIAL_INCLUDE_HINTS} NO_DEFAULT_PATH REQUIRED)
find_path(CIX_OPENJPEG_INCLUDE openjpeg.h HINTS ${CIX_SPATIAL_INCLUDE_HINTS}
    PATH_SUFFIXES openjpeg-2.5 openjpeg-2.4 openjpeg-2.3 NO_DEFAULT_PATH REQUIRED)
if(NOT CIX_CHARLS_LIBRARY)
    # CIX_CHARLS_LIBRARY is an empty cache entry by default. Discover into a
    # separate variable so CMake does not treat that empty cache value as a
    # completed find_library result.
    find_library(CIX_CHARLS_DISCOVERED_LIBRARY NAMES charls CharLS
        HINTS ${CIX_SPATIAL_LIBRARY_HINTS} NO_DEFAULT_PATH REQUIRED)
    set(CIX_CHARLS_LIBRARY "${CIX_CHARLS_DISCOVERED_LIBRARY}")
endif()
if(NOT CIX_OPENJPEG_LIBRARY)
    find_library(CIX_OPENJPEG_DISCOVERED_LIBRARY NAMES openjp2 openjpeg
        HINTS ${CIX_SPATIAL_LIBRARY_HINTS} NO_DEFAULT_PATH REQUIRED)
    set(CIX_OPENJPEG_LIBRARY "${CIX_OPENJPEG_DISCOVERED_LIBRARY}")
endif()
foreach(selected_library IN ITEMS CIX_CHARLS_LIBRARY CIX_OPENJPEG_LIBRARY)
    if(NOT IS_ABSOLUTE "${${selected_library}}" OR NOT EXISTS "${${selected_library}}")
        message(FATAL_ERROR "${selected_library} must be an existing absolute selected codec library")
    endif()
endforeach()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
add_library(cix_spatial_bridge SHARED cix_spatial_bridge.cpp)
target_compile_definitions(cix_spatial_bridge PRIVATE CIX_SPATIAL_BRIDGE_BUILDING)
target_include_directories(cix_spatial_bridge PRIVATE "${CIX_CHARLS_INCLUDE}" "${CIX_OPENJPEG_INCLUDE}")
target_link_libraries(cix_spatial_bridge PRIVATE "${CIX_CHARLS_LIBRARY}" "${CIX_OPENJPEG_LIBRARY}")
set_target_properties(cix_spatial_bridge PROPERTIES
    CXX_VISIBILITY_PRESET hidden
    VISIBILITY_INLINES_HIDDEN YES
    OUTPUT_NAME cix_spatial_bridge)
if(MSVC)
    target_compile_options(cix_spatial_bridge PRIVATE /O2 /fp:strict)
elseif(CMAKE_SYSTEM_NAME STREQUAL "Linux" AND
       CMAKE_SYSTEM_PROCESSOR MATCHES "^(x86_64|AMD64)$" AND
       CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang")
    # Preserve the exact flags used by the qualified Linux x86_64 bridge.
    target_compile_options(cix_spatial_bridge PRIVATE
        -O3 -fno-fast-math -ffp-contract=off -fno-strict-aliasing)
elseif(CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang|AppleClang")
    target_compile_options(cix_spatial_bridge PRIVATE
        -O3 -fno-fast-math -ffp-contract=off -fno-strict-aliasing)
else()
    message(FATAL_ERROR "CIX spatial bridge needs explicit safe optimization settings for this compiler")
endif()
if(UNIX AND NOT APPLE)
    set_target_properties(cix_spatial_bridge PROPERTIES INSTALL_RPATH "$ORIGIN")
elseif(APPLE)
    set_target_properties(cix_spatial_bridge PROPERTIES INSTALL_RPATH "@loader_path")
endif()
include(GNUInstallDirs)
install(TARGETS cix_spatial_bridge
    LIBRARY DESTINATION "${CMAKE_INSTALL_LIBDIR}"
    RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}"
    ARCHIVE DESTINATION "${CMAKE_INSTALL_LIBDIR}")
install(FILES cix_spatial_bridge.h DESTINATION "${CMAKE_INSTALL_INCLUDEDIR}/cix")
