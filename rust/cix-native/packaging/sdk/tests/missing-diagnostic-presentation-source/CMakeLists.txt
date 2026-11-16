cmake_minimum_required(VERSION 3.21)
project(cix_missing_diagnostic_presentation_source LANGUAGES CXX)

set(CIX_PRESENTATION_HEADER_DIR "" CACHE PATH "Exact staged candidate directory containing all three headers")
if(NOT IS_ABSOLUTE "${CIX_PRESENTATION_HEADER_DIR}")
    message(FATAL_ERROR "CIX_PRESENTATION_HEADER_DIR must be explicit and absolute")
endif()
foreach(header IN ITEMS cix.h cix.hpp cix_presentation.hpp)
    if(NOT EXISTS "${CIX_PRESENTATION_HEADER_DIR}/${header}")
        message(FATAL_ERROR "Missing selected source header: ${header}")
    endif()
endforeach()

add_executable(cix_missing_diagnostic_presentation_source main.cpp)
target_include_directories(cix_missing_diagnostic_presentation_source PRIVATE "${CIX_PRESENTATION_HEADER_DIR}")
target_compile_features(cix_missing_diagnostic_presentation_source PRIVATE cxx_std_17)
# C ABI definitions are test stubs in main.cpp. No CIX::native/provider linking or install.
enable_testing()
add_test(NAME cix_missing_diagnostic_presentation_source COMMAND cix_missing_diagnostic_presentation_source)
