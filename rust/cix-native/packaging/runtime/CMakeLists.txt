cmake_minimum_required(VERSION 3.21)
project(CIXRuntime VERSION 0.1.0 LANGUAGES NONE)

# The prebuilt providers use the explicit lib/cix layout on every target.
set(CMAKE_INSTALL_LIBDIR "lib" CACHE PATH "Runtime library installation directory")
include(GNUInstallDirs)

set(CIX_RUNTIME_CAPABILITY "full-engine" CACHE STRING
    "Installed capability: full-engine or native-only")
set_property(CACHE CIX_RUNTIME_CAPABILITY PROPERTY STRINGS full-engine native-only)
set(CIX_RUNTIME_CIX_EXECUTABLE "" CACHE FILEPATH
    "Absolute path to the selected prebuilt CIX executable")
set(CIX_RUNTIME_PAQ_V215_BRIDGE "" CACHE FILEPATH
    "Absolute path to the selected cix_paq_v215_bridge shared library")
set(CIX_RUNTIME_PAQ_V216_BRIDGE "" CACHE FILEPATH
    "Absolute path to the selected cix_paq_v216_bridge shared library")
set(CIX_RUNTIME_PAQ_JOINT_DISCOUNT_BRIDGE "" CACHE FILEPATH
    "Absolute path to the selected cix_paq_joint_discount_bridge shared library")
set(CIX_RUNTIME_PAQ_STORE_STATE_BRIDGE "" CACHE FILEPATH
    "Absolute path to the selected cix_paq_store_state_bridge shared library")
set(CIX_RUNTIME_JXL_BRIDGE "" CACHE FILEPATH
    "Absolute path to the selected cix_jxl_bridge shared library")
set(CIX_RUNTIME_SPATIAL_BRIDGE "" CACHE FILEPATH
    "Absolute path to the selected cix_spatial_bridge shared library")
set(CIX_RUNTIME_JXL_LIBRARY "" CACHE FILEPATH
    "Absolute path to selected libjxl.so.0.12.0 companion")
set(CIX_RUNTIME_JXL_THREADS_LIBRARY "" CACHE FILEPATH
    "Absolute path to selected libjxl_threads.so.0.12.0 companion")
set(CIX_RUNTIME_JXL_CMS_LIBRARY "" CACHE FILEPATH
    "Absolute path to selected libjxl_cms.so.0.12.0 companion")
set(CIX_RUNTIME_BROTLIDEC_LIBRARY "" CACHE FILEPATH
    "Absolute path to selected libbrotlidec.so.1.2.0 companion")
set(CIX_RUNTIME_BROTLIENC_LIBRARY "" CACHE FILEPATH
    "Absolute path to selected libbrotlienc.so.1.2.0 companion")
set(CIX_RUNTIME_BROTLICOMMON_LIBRARY "" CACHE FILEPATH
    "Absolute path to selected libbrotlicommon.so.1.2.0 companion")
set(CIX_RUNTIME_CHARLS_LIBRARY "" CACHE FILEPATH
    "Absolute path to selected libcharls.so.2.4.2 companion")
set(CIX_RUNTIME_OPENJPEG_LIBRARY "" CACHE FILEPATH
    "Absolute path to selected libopenjp2.so.2.5.0 companion")
set(CIX_RUNTIME_READELF "" CACHE FILEPATH
    "Absolute path to a selected readelf used only for Linux runtime input inspection")

if(NOT CIX_RUNTIME_CAPABILITY STREQUAL "full-engine" AND
   NOT CIX_RUNTIME_CAPABILITY STREQUAL "native-only")
  message(FATAL_ERROR "CIX_RUNTIME_CAPABILITY must be full-engine or native-only")
endif()
if(CIX_RUNTIME_CAPABILITY STREQUAL "full-engine" AND
   NOT CMAKE_SYSTEM_NAME STREQUAL "Linux")
  message(FATAL_ERROR "full-engine companion staging currently requires qualified ELF/Linux inputs")
endif()
if(NOT IS_ABSOLUTE "${CIX_RUNTIME_CIX_EXECUTABLE}" OR
   NOT EXISTS "${CIX_RUNTIME_CIX_EXECUTABLE}" OR
   IS_DIRECTORY "${CIX_RUNTIME_CIX_EXECUTABLE}")
  message(FATAL_ERROR "CIX_RUNTIME_CIX_EXECUTABLE must name an existing absolute regular prebuilt CIX executable")
endif()

# The names below exactly match full_engine::installed discovery at
# <prefix>/bin/cix plus <prefix>/lib/cix.  No PATH/environment/directory scan
# or build-host fallback is used by this installer.
if(WIN32)
  set(CIX_RUNTIME_SHARED_SUFFIX ".dll")
elseif(APPLE)
  set(CIX_RUNTIME_SHARED_SUFFIX ".dylib")
else()
  set(CIX_RUNTIME_SHARED_SUFFIX ".so")
endif()
set(CIX_RUNTIME_BRIDGES
  "${CIX_RUNTIME_PAQ_V215_BRIDGE}|libcix_paq_v215_bridge${CIX_RUNTIME_SHARED_SUFFIX}"
  "${CIX_RUNTIME_PAQ_V216_BRIDGE}|libcix_paq_v216_bridge${CIX_RUNTIME_SHARED_SUFFIX}"
  "${CIX_RUNTIME_PAQ_JOINT_DISCOUNT_BRIDGE}|libcix_paq_joint_discount_bridge${CIX_RUNTIME_SHARED_SUFFIX}"
  "${CIX_RUNTIME_PAQ_STORE_STATE_BRIDGE}|libcix_paq_store_state_bridge${CIX_RUNTIME_SHARED_SUFFIX}"
  "${CIX_RUNTIME_JXL_BRIDGE}|libcix_jxl_bridge${CIX_RUNTIME_SHARED_SUFFIX}"
  "${CIX_RUNTIME_SPATIAL_BRIDGE}|libcix_spatial_bridge${CIX_RUNTIME_SHARED_SUFFIX}")

if(CIX_RUNTIME_CAPABILITY STREQUAL "full-engine")
  foreach(pair IN LISTS CIX_RUNTIME_BRIDGES)
    string(REPLACE "|" ";" fields "${pair}")
    list(GET fields 0 source)
    if(NOT IS_ABSOLUTE "${source}" OR NOT EXISTS "${source}" OR IS_DIRECTORY "${source}")
      message(FATAL_ERROR "full-engine installation requires each explicit bridge path; missing or non-absolute: ${source}")
    endif()
  endforeach()
endif()

# Current companion layout is intentionally ELF/Linux-only.  Do not silently
# reuse these sonames on macOS or Windows: those targets need their own pinned
# bundle identities and loader qualification.
if(CIX_RUNTIME_CAPABILITY STREQUAL "full-engine")
  set(CIX_RUNTIME_COMPANIONS
    "${CIX_RUNTIME_JXL_LIBRARY}|libjxl.so.0.12.0|libjxl.so.0.12"
    "${CIX_RUNTIME_JXL_THREADS_LIBRARY}|libjxl_threads.so.0.12.0|libjxl_threads.so.0.12"
    "${CIX_RUNTIME_JXL_CMS_LIBRARY}|libjxl_cms.so.0.12.0|libjxl_cms.so.0.12"
    "${CIX_RUNTIME_BROTLIDEC_LIBRARY}|libbrotlidec.so.1.2.0|libbrotlidec.so.1"
    "${CIX_RUNTIME_BROTLIENC_LIBRARY}|libbrotlienc.so.1.2.0|libbrotlienc.so.1"
    "${CIX_RUNTIME_BROTLICOMMON_LIBRARY}|libbrotlicommon.so.1.2.0|libbrotlicommon.so.1"
    "${CIX_RUNTIME_CHARLS_LIBRARY}|libcharls.so.2.4.2|libcharls.so.2"
    "${CIX_RUNTIME_OPENJPEG_LIBRARY}|libopenjp2.so.2.5.0|libopenjp2.so.7")
  foreach(pair IN LISTS CIX_RUNTIME_COMPANIONS)
    string(REPLACE "|" ";" fields "${pair}")
    list(GET fields 0 source)
    if(NOT IS_ABSOLUTE "${source}" OR NOT EXISTS "${source}" OR IS_DIRECTORY "${source}")
      message(FATAL_ERROR "full-engine installation requires each explicit non-system companion path; missing or non-absolute: ${source}")
    endif()
  endforeach()
  # Inspect selected inputs without modifying them.  file(RPATH_CHECK) is
  # deliberately not used: on mismatch it can remove its FILE argument.
  if(NOT IS_ABSOLUTE "${CIX_RUNTIME_READELF}" OR NOT EXISTS "${CIX_RUNTIME_READELF}" OR IS_DIRECTORY "${CIX_RUNTIME_READELF}")
    message(FATAL_ERROR "full-engine Linux staging requires CIX_RUNTIME_READELF as an existing absolute read-only inspection tool")
  endif()
  set(CIX_RUNTIME_ORIGIN_RPATH_FILES
    "${CIX_RUNTIME_JXL_BRIDGE}"
    "${CIX_RUNTIME_SPATIAL_BRIDGE}"
    "${CIX_RUNTIME_JXL_LIBRARY}"
    "${CIX_RUNTIME_JXL_THREADS_LIBRARY}"
    "${CIX_RUNTIME_JXL_CMS_LIBRARY}"
    "${CIX_RUNTIME_BROTLIDEC_LIBRARY}"
    "${CIX_RUNTIME_BROTLIENC_LIBRARY}"
    "${CIX_RUNTIME_BROTLICOMMON_LIBRARY}")
  foreach(candidate IN LISTS CIX_RUNTIME_ORIGIN_RPATH_FILES)
    execute_process(COMMAND "${CIX_RUNTIME_READELF}" -d "${candidate}"
      RESULT_VARIABLE inspection_status OUTPUT_VARIABLE inspection_output ERROR_VARIABLE inspection_error)
    if(NOT inspection_status EQUAL 0)
      message(FATAL_ERROR "readelf inspection failed for ${candidate}: ${inspection_error}")
    endif()
    string(REGEX MATCH "\\(RUNPATH\\)[^\n]*\\[([^]]*)\\]" runpath_record "${inspection_output}")
    if(NOT runpath_record OR NOT CMAKE_MATCH_1 STREQUAL "$ORIGIN")
      message(FATAL_ERROR "selected full-engine runtime input must have exactly $ORIGIN RUNPATH: ${candidate}")
    endif()
  endforeach()
endif()

if(WIN32)
  install(PROGRAMS "${CIX_RUNTIME_CIX_EXECUTABLE}" DESTINATION "${CMAKE_INSTALL_BINDIR}" RENAME cix.exe)
  install(CODE "execute_process(COMMAND \"${CMAKE_COMMAND}\" -E copy \"\$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/${CMAKE_INSTALL_BINDIR}/cix.exe\" \"\$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/${CMAKE_INSTALL_BINDIR}/uncix.exe\" RESULT_VARIABLE result)\nif(NOT result EQUAL 0)\n  message(FATAL_ERROR \"cannot copy uncix Windows runtime alias\")\nendif()")
  install(CODE "execute_process(COMMAND \"${CMAKE_COMMAND}\" -E copy \"\$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/${CMAKE_INSTALL_BINDIR}/cix.exe\" \"\$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/${CMAKE_INSTALL_BINDIR}/cixcat.exe\" RESULT_VARIABLE result)\nif(NOT result EQUAL 0)\n  message(FATAL_ERROR \"cannot copy cixcat Windows runtime alias\")\nendif()")
else()
  install(PROGRAMS "${CIX_RUNTIME_CIX_EXECUTABLE}" DESTINATION "${CMAKE_INSTALL_BINDIR}" RENAME cix)
  install(CODE "execute_process(COMMAND \"${CMAKE_COMMAND}\" -E create_symlink cix \"\$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/${CMAKE_INSTALL_BINDIR}/uncix\" RESULT_VARIABLE result)\nif(NOT result EQUAL 0)\n  message(FATAL_ERROR \"cannot create uncix runtime alias\")\nendif()")
  install(CODE "execute_process(COMMAND \"${CMAKE_COMMAND}\" -E create_symlink cix \"\$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/${CMAKE_INSTALL_BINDIR}/cixcat\" RESULT_VARIABLE result)\nif(NOT result EQUAL 0)\n  message(FATAL_ERROR \"cannot create cixcat runtime alias\")\nendif()")
endif()

if(CIX_RUNTIME_CAPABILITY STREQUAL "full-engine")
  foreach(pair IN LISTS CIX_RUNTIME_BRIDGES)
    string(REPLACE "|" ";" fields "${pair}")
    list(GET fields 0 source)
    list(GET fields 1 installed_name)
    install(FILES "${source}" DESTINATION "lib/cix" RENAME "${installed_name}")
  endforeach()
  foreach(pair IN LISTS CIX_RUNTIME_COMPANIONS)
    string(REPLACE "|" ";" fields "${pair}")
    list(GET fields 0 source)
    list(GET fields 1 installed_name)
    list(GET fields 2 soname)
    install(FILES "${source}" DESTINATION "lib/cix" RENAME "${installed_name}")
    install(CODE "execute_process(COMMAND \"${CMAKE_COMMAND}\" -E create_symlink ${installed_name} \"\$ENV{DESTDIR}${CMAKE_INSTALL_PREFIX}/lib/cix/${soname}\" RESULT_VARIABLE result)\nif(NOT result EQUAL 0)\n  message(FATAL_ERROR \"cannot create runtime companion SONAME link ${soname}\")\nendif()")
  endforeach()
endif()
install(FILES "${CMAKE_CURRENT_LIST_DIR}/README.md" DESTINATION "${CMAKE_INSTALL_DOCDIR}")
