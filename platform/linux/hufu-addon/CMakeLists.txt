# 虎符（hufu-ime）fcitx5 addon 构建/安装
#
# 依赖：CMake 3.20+、C++20 编译器、提供 CMake 包 Fcitx5Core 的 fcitx5 开发文件
#（Arch: fcitx5；Fedora: fcitx5-devel；Debian/Ubuntu: libfcitx5core-dev）、Rust。
#
# 用法：
#   cmake -S platform/linux/hufu-addon -B platform/linux/build \
#       -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr
#   cmake --build platform/linux/build -j
#   sudo cmake --install platform/linux/build
cmake_minimum_required(VERSION 3.20)
project(hufu-addon VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(Fcitx5Core REQUIRED)

# Rust 静态库（客户端层；cargo 自身增量，故每次构建都调用）。
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(HUFU_CARGO_PROFILE "debug")
    set(HUFU_CARGO_FLAGS "")
else()
    set(HUFU_CARGO_PROFILE "release")
    set(HUFU_CARGO_FLAGS "--release")
endif()
set(HUFU_WORKSPACE "${CMAKE_CURRENT_SOURCE_DIR}/..")
set(HUFU_RUST_LIB
    "${HUFU_WORKSPACE}/target/${HUFU_CARGO_PROFILE}/libhufu_fcitx5_client.a")
add_custom_target(hufu-rust ALL
    # --offline：Cargo.lock 已提交，避免联网刷新索引（离线环境会挂起）
    COMMAND cargo build --offline -p hufu-fcitx5-client ${HUFU_CARGO_FLAGS}
    WORKING_DIRECTORY "${HUFU_WORKSPACE}"
    COMMENT "cargo build -p hufu-fcitx5-client (${HUFU_CARGO_PROFILE})"
    VERBATIM)

add_library(hufu SHARED shell/hufu.cpp shell/hufu_abi.h)
add_dependencies(hufu hufu-rust)
target_include_directories(hufu PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/shell")
target_link_libraries(hufu PRIVATE Fcitx5::Core "${HUFU_RUST_LIB}" dl pthread m)
set_target_properties(hufu PROPERTIES OUTPUT_NAME "hufu")

install(TARGETS hufu LIBRARY DESTINATION lib/fcitx5)
install(FILES conf/hufu.addon.conf
        DESTINATION share/fcitx5/addon
        RENAME hufu.conf)
install(FILES conf/hufu.inputmethod.conf
        DESTINATION share/fcitx5/inputmethod
        RENAME hufu.conf)
