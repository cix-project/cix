cmake_minimum_required(VERSION 3.20)
project(cix_paq_joint_discount_bridge LANGUAGES C CXX)

# The source tree is supplied by the distribution build. It is never copied,
# patched, or discovered from a development/research checkout at runtime.
set(CIX_PAQ_SOURCE_ROOT "" CACHE PATH "Unchanged PAQ joint_discount source root")
if(NOT IS_DIRECTORY "${CIX_PAQ_SOURCE_ROOT}/src")
    message(FATAL_ERROR "Set CIX_PAQ_SOURCE_ROOT to the packaged PAQ joint_discount source root")
endif()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

file(GLOB_RECURSE PAQ_CPP CONFIGURE_DEPENDS
    "${CIX_PAQ_SOURCE_ROOT}/src/*.cpp")
set(PAQ_ZLIB
    "${CIX_PAQ_SOURCE_ROOT}/zlib/adler32.c"
    "${CIX_PAQ_SOURCE_ROOT}/zlib/crc32.c"
    "${CIX_PAQ_SOURCE_ROOT}/zlib/deflate.c"
    "${CIX_PAQ_SOURCE_ROOT}/zlib/gzlib.c"
    "${CIX_PAQ_SOURCE_ROOT}/zlib/inffast.c"
    "${CIX_PAQ_SOURCE_ROOT}/zlib/inflate.c"
    "${CIX_PAQ_SOURCE_ROOT}/zlib/inftrees.c"
    "${CIX_PAQ_SOURCE_ROOT}/zlib/trees.c"
    "${CIX_PAQ_SOURCE_ROOT}/zlib/zutil.c")

add_library(cix_paq_joint_discount_bridge SHARED paq_joint_discount_bridge.cpp ${PAQ_CPP} ${PAQ_ZLIB})
set_target_properties(cix_paq_joint_discount_bridge PROPERTIES
    C_VISIBILITY_PRESET hidden
    CXX_VISIBILITY_PRESET hidden
    VISIBILITY_INLINES_HIDDEN YES
    OUTPUT_NAME cix_paq_joint_discount_bridge)

# PAQ ships a command-line entrypoint. Macro-renaming it at compilation keeps
# the upstream sources byte-identical while allowing the CIX bridge to link it.
set_source_files_properties("${CIX_PAQ_SOURCE_ROOT}/src/paq8px.cpp"
    PROPERTIES COMPILE_DEFINITIONS "main=cix_paq_joint_discount_upstream_main")

target_compile_definitions(cix_paq_joint_discount_bridge PRIVATE CIX_PAQ_JOINT_DISCOUNT_BUILDING NDEBUG)
target_include_directories(cix_paq_joint_discount_bridge PRIVATE
    "${CIX_PAQ_SOURCE_ROOT}/src"
    "${CIX_PAQ_SOURCE_ROOT}/zlib")
if(MSVC)
    target_compile_options(cix_paq_joint_discount_bridge PRIVATE
        /O2 /fp:strict $<$<COMPILE_LANGUAGE:CXX>:/GR->)
elseif(CMAKE_SYSTEM_NAME STREQUAL "Linux" AND
       CMAKE_SYSTEM_PROCESSOR MATCHES "^(x86_64|AMD64)$" AND
       CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang")
    # Preserve the exact flags used by the qualified Linux x86_64 bridge.
    target_compile_options(cix_paq_joint_discount_bridge PRIVATE
        -O3 -fno-fast-math -ffp-contract=off -fno-strict-aliasing
        $<$<COMPILE_LANGUAGE:CXX>:-fno-rtti>)
elseif(CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang|AppleClang")
    target_compile_options(cix_paq_joint_discount_bridge PRIVATE
        -O3 -fno-fast-math -ffp-contract=off -fno-strict-aliasing
        $<$<COMPILE_LANGUAGE:CXX>:-fno-rtti>)
else()
    message(FATAL_ERROR "CIX PAQ bridge needs explicit safe optimization and RTTI settings for this compiler")
endif()
if(UNIX)
    target_compile_options(cix_paq_joint_discount_bridge PRIVATE
        $<$<COMPILE_LANGUAGE:C>:-include>
        $<$<COMPILE_LANGUAGE:C>:unistd.h>)
endif()
if(UNIX AND NOT APPLE)
    # Each variant is a separate DSO. Bind its upstream globals locally so a
    # later v216/joint-discount/store-state DSO cannot interpose them.
    target_link_options(cix_paq_joint_discount_bridge PRIVATE -Wl,-Bsymbolic "-Wl,--version-script=${CMAKE_CURRENT_SOURCE_DIR}/exports.map")
endif()

include(GNUInstallDirs)
install(TARGETS cix_paq_joint_discount_bridge
    LIBRARY DESTINATION "${CMAKE_INSTALL_LIBDIR}"
    RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}"
    ARCHIVE DESTINATION "${CMAKE_INSTALL_LIBDIR}")
install(FILES paq_joint_discount_bridge.h DESTINATION "${CMAKE_INSTALL_INCLUDEDIR}/cix")
