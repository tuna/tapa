cmake_minimum_required(VERSION 3.20)
project(frt_vrt LANGUAGES CXX)

find_package(vrt REQUIRED CONFIG)
include(GNUInstallDirs)

add_library(frt_vrt SHARED bridge.cpp)
target_compile_features(frt_vrt PRIVATE cxx_std_17)
target_compile_options(frt_vrt PRIVATE -Wall -Wextra -Werror)
target_link_libraries(frt_vrt PRIVATE vrt::vrt)
install(TARGETS frt_vrt LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR})
