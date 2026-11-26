cmake_minimum_required(VERSION 3.20)

project(TigerClawSentenceNative LANGUAGES C CXX)

set(CMAKE_POSITION_INDEPENDENT_CODE ON)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(GGML_STATIC ON CACHE BOOL "" FORCE)
set(GGML_BACKEND_DL OFF CACHE BOOL "" FORCE)
set(GGML_NATIVE OFF CACHE BOOL "" FORCE)
set(GGML_OPENMP OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_COMMON OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_TOOLS OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_SERVER OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_APP OFF CACHE BOOL "" FORCE)
set(LLAMA_BUILD_UI OFF CACHE BOOL "" FORCE)

add_subdirectory(
    "${CMAKE_CURRENT_SOURCE_DIR}/../../third_party/llama.cpp"
    "${CMAKE_CURRENT_BINARY_DIR}/llama.cpp")

add_executable(TigerClaw.Sentence.Host WIN32
    SentenceHost.cpp
    SentenceQwenNative.cpp
    SentenceQwenNative.h)

target_compile_features(TigerClaw.Sentence.Host PRIVATE cxx_std_17)
target_compile_definitions(TigerClaw.Sentence.Host PRIVATE TIGERCLAW_SENTENCE_NATIVE_STATIC)
target_include_directories(TigerClaw.Sentence.Host PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}/../../third_party/llama.cpp/include"
    "${CMAKE_CURRENT_SOURCE_DIR}/../../third_party/llama.cpp/vendor")
target_link_libraries(TigerClaw.Sentence.Host PRIVATE llama shell32)

if(MSVC)
    target_compile_options(TigerClaw.Sentence.Host PRIVATE /utf-8 /EHsc)
endif()

set_target_properties(TigerClaw.Sentence.Host PROPERTIES
    OUTPUT_NAME "TigerClaw.Sentence")

# Independent offline experiment; never included in the default/publish build.
add_executable(TigerClaw.Sentence.FullPinyinProbe WIN32 EXCLUDE_FROM_ALL
    SentenceHost.cpp SentenceQwenNative.cpp SentenceQwenNative.h)
target_compile_features(TigerClaw.Sentence.FullPinyinProbe PRIVATE cxx_std_17)
target_compile_definitions(TigerClaw.Sentence.FullPinyinProbe PRIVATE
    TIGERCLAW_SENTENCE_NATIVE_STATIC TIGERCLAW_SENTENCE_MAX_CANDIDATES=10
    TIGERCLAW_SENTENCE_CONTEXT_SIZE=4096 TIGERCLAW_SENTENCE_BATCH_SIZE=2048
    TIGERCLAW_SENTENCE_THREADS=4)
target_include_directories(TigerClaw.Sentence.FullPinyinProbe PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}/../../third_party/llama.cpp/include"
    "${CMAKE_CURRENT_SOURCE_DIR}/../../third_party/llama.cpp/vendor")
target_link_libraries(TigerClaw.Sentence.FullPinyinProbe PRIVATE llama shell32)
if(MSVC)
    target_compile_options(TigerClaw.Sentence.FullPinyinProbe PRIVATE /utf-8 /EHsc)
endif()

# Explicitly requested diagnostic target; never packaged or run by normal builds.
add_executable(TigerClaw.Sentence.CancelProbe EXCLUDE_FROM_ALL
    tests/CancelProbe.cpp SentenceQwenNative.cpp SentenceQwenNative.h)
target_compile_features(TigerClaw.Sentence.CancelProbe PRIVATE cxx_std_17)
target_compile_definitions(TigerClaw.Sentence.CancelProbe PRIVATE TIGERCLAW_SENTENCE_NATIVE_STATIC)
target_include_directories(TigerClaw.Sentence.CancelProbe PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}"
    "${CMAKE_CURRENT_SOURCE_DIR}/../../third_party/llama.cpp/include")
target_link_libraries(TigerClaw.Sentence.CancelProbe PRIVATE llama)
if(MSVC)
    target_compile_options(TigerClaw.Sentence.CancelProbe PRIVATE /utf-8 /EHsc)
endif()
