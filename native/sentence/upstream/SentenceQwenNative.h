#pragma once

#include <cstddef>
#include <cstdint>

// Production remains Top-5. Explicit offline targets may use a larger batch.
#ifndef TIGERCLAW_SENTENCE_MAX_CANDIDATES
#define TIGERCLAW_SENTENCE_MAX_CANDIDATES 5
#endif
#ifndef TIGERCLAW_SENTENCE_CONTEXT_SIZE
#define TIGERCLAW_SENTENCE_CONTEXT_SIZE 512
#endif
#ifndef TIGERCLAW_SENTENCE_BATCH_SIZE
#define TIGERCLAW_SENTENCE_BATCH_SIZE 512
#endif
#ifndef TIGERCLAW_SENTENCE_THREADS
#define TIGERCLAW_SENTENCE_THREADS 0
#endif

#if defined(_WIN32)
#if defined(TIGERCLAW_SENTENCE_NATIVE_STATIC)
#define TCS_API extern "C"
#elif defined(TIGERCLAW_SENTENCE_NATIVE_EXPORTS)
#define TCS_API extern "C" __declspec(dllexport)
#else
#define TCS_API extern "C" __declspec(dllimport)
#endif
#define TCS_CALL __cdecl
#else
#define TCS_API extern "C" __attribute__((visibility("default")))
#define TCS_CALL
#endif

TCS_API int TCS_CALL tcs_create_from_file(
    const char* modelPathUtf8,
    void** scorer);

TCS_API int TCS_CALL tcs_score(
    void* scorer,
    const char* const* candidatesUtf8,
    int32_t candidateCount,
    double* scores);

// Callback must be thread-safe and must not throw.
using tcs_abort_callback = bool (TCS_CALL *)(void*);
TCS_API int TCS_CALL tcs_score_cancellable(
    void* scorer, const char* const* candidatesUtf8, int32_t candidateCount,
    double* scores, tcs_abort_callback abort, void* abortContext);

TCS_API void TCS_CALL tcs_destroy(void* scorer);

TCS_API const char* TCS_CALL tcs_last_error();
