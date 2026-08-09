cmake_minimum_required(VERSION 3.16)
project(goosy_cpp LANGUAGES CXX)

add_library(goosy_cpp INTERFACE)
target_include_directories(goosy_cpp INTERFACE ${CMAKE_CURRENT_SOURCE_DIR} ../include)
target_compile_features(goosy_cpp INTERFACE cxx_std_17)

# Link the Rust artifact built by `cargo build --release`:
#   target/release/libgoosy.dylib (macOS)
#   target/release/libgoosy.so (Linux)
#   target/release/goosy.dll (Windows)
