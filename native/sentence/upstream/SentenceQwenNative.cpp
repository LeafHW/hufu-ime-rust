#include "SentenceQwenNative.h"

#include "llama.h"

#include <algorithm>
#include <atomic>
#include <cmath>
#include <exception>
#include <memory>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

namespace
{
    constexpr int32_t MaximumCandidates = TIGERCLAW_SENTENCE_MAX_CANDIDATES;
    constexpr uint32_t ContextSize = TIGERCLAW_SENTENCE_CONTEXT_SIZE;
    constexpr uint32_t BatchSize = TIGERCLAW_SENTENCE_BATCH_SIZE;

    thread_local std::string LastError;

    void SilentLog(ggml_log_level, const char*, void*)
    {
    }

    class BackendLifetime
    {
    public:
        BackendLifetime()
        {
            llama_log_set(SilentLog, nullptr);
            llama_backend_init();
        }

        ~BackendLifetime()
        {
            llama_backend_free();
        }
    };

    BackendLifetime& Backend()
    {
        static BackendLifetime lifetime;
        return lifetime;
    }

    struct ModelDeleter
    {
        void operator()(llama_model* model) const
        {
            llama_model_free(model);
        }
    };

    struct ContextDeleter
    {
        void operator()(llama_context* context) const
        {
            llama_free(context);
        }
    };

    using ModelPointer = std::unique_ptr<llama_model, ModelDeleter>;
    using ContextPointer = std::unique_ptr<llama_context, ContextDeleter>;

    class Scorer
    {
    public:
        Scorer(llama_model* model, int32_t threadCount)
            : _model(model),
              _vocabulary(llama_model_get_vocab(model)),
              _vocabularySize(llama_vocab_n_tokens(_vocabulary)),
              _threadCount(std::max(1, threadCount))
        {
            llama_context_params contextParams = llama_context_default_params();
            contextParams.n_ctx = ContextSize;
            contextParams.n_batch = BatchSize;
            contextParams.n_ubatch = BatchSize;
            contextParams.n_seq_max = MaximumCandidates;
            contextParams.n_outputs_max = BatchSize;
            contextParams.n_outputs_max_per_seq = ContextSize;
            contextParams.n_threads = _threadCount;
            contextParams.n_threads_batch = _threadCount;
            contextParams.no_perf = true;
            contextParams.kv_unified = true;
            _context.reset(llama_init_from_model(_model.get(), contextParams));
            if (!_context)
            {
                throw std::runtime_error("cannot create Qwen context");
            }
        }

        std::vector<double> Score(const std::vector<std::string>& candidates, tcs_abort_callback abort, void* abortContext)
        {
            if (candidates.empty() || candidates.size() > MaximumCandidates)
            {
                throw std::runtime_error("candidate count must be between 1 and " + std::to_string(MaximumCandidates));
            }

            _abort = abort;
            _abortContext = abortContext;
            llama_set_abort_callback(_context.get(), abort, abortContext);
            struct ResetAbort
            {
                Scorer& owner;
                ~ResetAbort()
                {
                    llama_set_abort_callback(owner._context.get(), nullptr, nullptr);
                    owner._abort = nullptr;
                    owner._abortContext = nullptr;
                }
            } resetAbort{*this};
            CheckCanceled();
            const llama_token bos = llama_vocab_bos(_vocabulary);
            const llama_token eos = llama_vocab_eos(_vocabulary);
            if (bos == LLAMA_TOKEN_NULL || eos == LLAMA_TOKEN_NULL)
            {
                throw std::runtime_error("model does not define BOS and EOS tokens");
            }

            std::vector<std::vector<llama_token>> sequences;
            sequences.reserve(candidates.size());
            for (const std::string& candidate : candidates)
            {
                CheckCanceled();
                if (candidate.empty())
                {
                    throw std::runtime_error("candidate text must not be empty");
                }
                std::vector<llama_token> sequence { bos };
                std::vector<llama_token> textTokens = Tokenize(candidate);
                sequence.insert(sequence.end(), textTokens.begin(), textTokens.end());
                sequence.push_back(eos);
                if (sequence.size() > ContextSize)
                {
                    throw std::runtime_error("candidate exceeds the Qwen context size");
                }
                sequences.push_back(std::move(sequence));
            }

            const std::size_t commonPrefix = FindCommonPrefix(sequences);
            int32_t inputCount = static_cast<int32_t>(commonPrefix);
            for (const std::vector<llama_token>& sequence : sequences)
            {
                inputCount += static_cast<int32_t>(sequence.size() - 1 - commonPrefix);
            }
            if (inputCount <= 0 || inputCount > static_cast<int32_t>(BatchSize))
            {
                throw std::runtime_error("candidate batch exceeds the Qwen batch size");
            }

            llama_batch batch = llama_batch_init(
                inputCount, 0, static_cast<int32_t>(sequences.size()));
            try
            {
                for (std::size_t position = 0; position < commonPrefix; ++position)
                {
                    BatchAddShared(
                        batch,
                        sequences.front()[position],
                        static_cast<llama_pos>(position),
                        static_cast<int32_t>(sequences.size()),
                        true);
                }
                for (std::size_t sequenceIndex = 0; sequenceIndex < sequences.size(); ++sequenceIndex)
                {
                    const std::vector<llama_token>& sequence = sequences[sequenceIndex];
                    for (std::size_t position = commonPrefix; position + 1 < sequence.size(); ++position)
                    {
                        BatchAdd(
                            batch,
                            sequence[position],
                            static_cast<llama_pos>(position),
                            static_cast<llama_seq_id>(sequenceIndex),
                            true);
                    }
                }

                CheckCanceled();
                llama_memory_clear(llama_get_memory(_context.get()), true);
                const int32_t decodeResult = llama_decode(_context.get(), batch);
                if (decodeResult != 0)
                {
                    throw std::runtime_error(
                        "llama_decode failed: " + std::to_string(decodeResult));
                }

                std::vector<double> scores = CollectScores(sequences, commonPrefix);
                llama_batch_free(batch);
                return scores;
            }
            catch (...)
            {
                llama_batch_free(batch);
                throw;
            }
        }

    private:
        struct OutputTarget
        {
            int32_t OutputIndex;
            std::size_t SequenceIndex;
            llama_token Token;
        };

        std::vector<llama_token> Tokenize(const std::string& text) const
        {
            const int32_t required = -llama_tokenize(
                _vocabulary,
                text.data(),
                static_cast<int32_t>(text.size()),
                nullptr,
                0,
                false,
                true);
            if (required <= 0)
            {
                throw std::runtime_error("Qwen tokenization failed");
            }
            std::vector<llama_token> tokens(required);
            const int32_t count = llama_tokenize(
                _vocabulary,
                text.data(),
                static_cast<int32_t>(text.size()),
                tokens.data(),
                static_cast<int32_t>(tokens.size()),
                false,
                true);
            if (count != required)
            {
                throw std::runtime_error("Qwen tokenization returned an unexpected size");
            }
            return tokens;
        }

        static std::size_t FindCommonPrefix(
            const std::vector<std::vector<llama_token>>& sequences)
        {
            std::size_t commonPrefix = sequences.front().size() - 1;
            for (const std::vector<llama_token>& sequence : sequences)
            {
                commonPrefix = std::min(commonPrefix, sequence.size() - 1);
            }
            for (std::size_t position = 0; position < commonPrefix; ++position)
            {
                const llama_token token = sequences.front()[position];
                for (std::size_t sequenceIndex = 1; sequenceIndex < sequences.size(); ++sequenceIndex)
                {
                    if (sequences[sequenceIndex][position] != token)
                    {
                        return position;
                    }
                }
            }
            if (commonPrefix == 0)
            {
                throw std::runtime_error("candidate sequences do not share BOS");
            }
            return commonPrefix;
        }

        static void BatchAdd(
            llama_batch& batch,
            llama_token token,
            llama_pos position,
            llama_seq_id sequence,
            bool output)
        {
            const int32_t index = batch.n_tokens++;
            batch.token[index] = token;
            batch.pos[index] = position;
            batch.n_seq_id[index] = 1;
            batch.seq_id[index][0] = sequence;
            batch.logits[index] = output ? 1 : 0;
        }

        static void BatchAddShared(
            llama_batch& batch,
            llama_token token,
            llama_pos position,
            int32_t sequenceCount,
            bool output)
        {
            const int32_t index = batch.n_tokens++;
            batch.token[index] = token;
            batch.pos[index] = position;
            batch.n_seq_id[index] = sequenceCount;
            for (int32_t sequence = 0; sequence < sequenceCount; ++sequence)
            {
                batch.seq_id[index][sequence] = sequence;
            }
            batch.logits[index] = output ? 1 : 0;
        }

        double LogProbability(const float* logits, llama_token target) const
        {
            float maximum = logits[0];
            for (int32_t index = 1; index < _vocabularySize; ++index)
            {
                maximum = std::max(maximum, logits[index]);
            }
            double sum = 0.0;
            for (int32_t index = 0; index < _vocabularySize; ++index)
            {
                sum += std::exp(static_cast<double>(logits[index] - maximum));
            }
            return static_cast<double>(logits[target] - maximum) - std::log(sum);
        }

        std::vector<double> CollectScores(
            const std::vector<std::vector<llama_token>>& sequences,
            std::size_t commonPrefix) const
        {
            std::vector<OutputTarget> targets;
            for (std::size_t position = 0; position < commonPrefix; ++position)
            {
                for (std::size_t sequenceIndex = 0; sequenceIndex < sequences.size(); ++sequenceIndex)
                {
                    targets.push_back({
                        static_cast<int32_t>(position),
                        sequenceIndex,
                        sequences[sequenceIndex][position + 1] });
                }
            }
            int32_t outputIndex = static_cast<int32_t>(commonPrefix);
            for (std::size_t sequenceIndex = 0; sequenceIndex < sequences.size(); ++sequenceIndex)
            {
                const std::vector<llama_token>& sequence = sequences[sequenceIndex];
                for (std::size_t position = commonPrefix; position + 1 < sequence.size(); ++position)
                {
                    targets.push_back({ outputIndex++, sequenceIndex, sequence[position + 1] });
                }
            }

            std::vector<double> tokenScores(targets.size());
            std::atomic<std::size_t> nextTarget(0);
            const int32_t workerCount = std::max(
                1,
                std::min(_threadCount, static_cast<int32_t>(targets.size())));
            std::vector<std::thread> workers;
            workers.reserve(workerCount);
            for (int32_t threadIndex = 0; threadIndex < workerCount; ++threadIndex)
            {
                workers.emplace_back([&]()
                {
                    while (true)
                    {
                        if (IsCanceled()) return;
                        const std::size_t targetIndex = nextTarget.fetch_add(1);
                        if (targetIndex >= targets.size())
                        {
                            return;
                        }
                        const OutputTarget& target = targets[targetIndex];
                        const float* logits = llama_get_logits(_context.get()) +
                            static_cast<std::size_t>(target.OutputIndex) * _vocabularySize;
                        tokenScores[targetIndex] = LogProbability(logits, target.Token);
                    }
                });
            }
            for (std::thread& worker : workers)
            {
                worker.join();
            }

            CheckCanceled();
            std::vector<double> scores(sequences.size(), 0.0);
            for (std::size_t targetIndex = 0; targetIndex < targets.size(); ++targetIndex)
            {
                scores[targets[targetIndex].SequenceIndex] += tokenScores[targetIndex];
            }
            return scores;
        }

        bool IsCanceled() const { return _abort != nullptr && _abort(_abortContext); }
        void CheckCanceled() const
        {
            if (IsCanceled()) throw std::runtime_error("sentence request canceled");
        }
        tcs_abort_callback _abort = nullptr;
        void* _abortContext = nullptr;
        ModelPointer _model;
        const llama_vocab* _vocabulary;
        int32_t _vocabularySize;
        int32_t _threadCount;
        ContextPointer _context;
    };

    llama_model_params DirectModelParameters()
    {
        llama_model_params parameters = llama_model_default_params();
        parameters.n_gpu_layers = 0;
        parameters.use_extra_bufts = false;
        return parameters;
    }

    template<typename Action>
    int Guard(Action action)
    {
        try
        {
            LastError.clear();
            Backend();
            action();
            return 0;
        }
        catch (const std::exception& error)
        {
            LastError = error.what();
        }
        catch (...)
        {
            LastError = "unknown native Qwen error";
        }
        return 1;
    }
}

int TCS_CALL tcs_create_from_file(
    const char* modelPathUtf8,
    void** scorer)
{
    return Guard([&]()
    {
        if (modelPathUtf8 == nullptr || *modelPathUtf8 == '\0' || scorer == nullptr)
        {
            throw std::runtime_error("Qwen model path is required");
        }
        *scorer = nullptr;
        llama_model* model = llama_model_load_from_file(
            modelPathUtf8, DirectModelParameters());
        if (model == nullptr)
        {
            throw std::runtime_error("cannot load Qwen model");
        }
        const int32_t logicalProcessors = static_cast<int32_t>(
            std::max(1u, std::thread::hardware_concurrency()));
        const int32_t threadCount = TIGERCLAW_SENTENCE_THREADS > 0
            ? TIGERCLAW_SENTENCE_THREADS : std::max(1, logicalProcessors / 2);
        *scorer = new Scorer(model, threadCount);
    });
}

int TCS_CALL tcs_score_cancellable(
    void* scorer,
    const char* const* candidatesUtf8,
    int32_t candidateCount,
    double* scores, tcs_abort_callback abort, void* abortContext)
{
    return Guard([&]()
    {
        if (scorer == nullptr || candidatesUtf8 == nullptr || scores == nullptr)
        {
            throw std::runtime_error("Qwen scorer arguments are required");
        }
        if (candidateCount < 1 || candidateCount > MaximumCandidates)
        {
            throw std::runtime_error("candidate count must be between 1 and " + std::to_string(MaximumCandidates));
        }
        std::vector<std::string> candidates;
        candidates.reserve(candidateCount);
        for (int32_t index = 0; index < candidateCount; ++index)
        {
            if (candidatesUtf8[index] == nullptr)
            {
                throw std::runtime_error("candidate text is required");
            }
            candidates.emplace_back(candidatesUtf8[index]);
        }
        const std::vector<double> result = static_cast<Scorer*>(scorer)->Score(candidates, abort, abortContext);
        std::copy(result.begin(), result.end(), scores);
    });
}

int TCS_CALL tcs_score(void* scorer, const char* const* candidatesUtf8,
    int32_t candidateCount, double* scores)
{
    return tcs_score_cancellable(scorer, candidatesUtf8, candidateCount, scores, nullptr, nullptr);
}

void TCS_CALL tcs_destroy(void* scorer)
{
    delete static_cast<Scorer*>(scorer);
}

const char* TCS_CALL tcs_last_error()
{
    return LastError.c_str();
}
