// Windows launcher for rust-lld using one coherent GNU CRT/library set.
// Rustup's current dllcrt2.o and independently supplied MinGW CRT must not mix.
#include <windows.h>
#include <shellapi.h>
#pragma comment(lib, "shell32.lib")
#include <string>
#include <vector>
#include <cwchar>
#include <fstream>

static std::vector<std::wstring> arguments(int argc, wchar_t** argv) {
    std::vector<std::wstring> result;
    for (int i = 1; i < argc; ++i) {
        if (argv[i][0] != L'@') { result.emplace_back(argv[i]); continue; }
        std::ifstream input(argv[i] + 1, std::ios::binary);
        const std::string bytes((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
        std::wstring text;
        if (bytes.size() >= 2 && static_cast<unsigned char>(bytes[0]) == 0xff && static_cast<unsigned char>(bytes[1]) == 0xfe) {
            text.assign(reinterpret_cast<const wchar_t*>(bytes.data() + 2), (bytes.size() - 2) / 2);
        } else {
            const int count = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, bytes.data(), static_cast<int>(bytes.size()), nullptr, 0);
            if (count <= 0) { fwprintf(stderr, L"Invalid linker response file.\n"); return {}; }
            text.resize(count);
            MultiByteToWideChar(CP_UTF8, 0, bytes.data(), static_cast<int>(bytes.size()), text.data(), count);
        }
        // rustc uses Windows response quoting on this host, including literal
        // backslashes inside ordinary paths. Normalize newlines as separators.
        for (auto& ch : text) if (ch == L'\n' || ch == L'\r') ch = L' ';
        int count = 0;
        const auto parsed = CommandLineToArgvW((L"linker " + text).c_str(), &count);
        if (!parsed) return {};
        for (int n = 1; n < count; ++n) result.emplace_back(parsed[n]);
        LocalFree(parsed);
    }
    return result;
}

static std::wstring environment(const wchar_t* name) {
    DWORD size = GetEnvironmentVariableW(name, nullptr, 0);
    if (!size) return {};
    std::wstring value(size, L'\0');
    GetEnvironmentVariableW(name, value.data(), size);
    value.resize(size - 1);
    return value;
}

static std::wstring quote(const std::wstring& value) {
    std::wstring result = L"\"";
    unsigned slashes = 0;
    for (wchar_t ch : value) {
        if (ch == L'\\') { ++slashes; continue; }
        result.append(ch == L'"' ? slashes * 2 + 1 : slashes, L'\\');
        result += ch;
        slashes = 0;
    }
    result.append(slashes * 2, L'\\');
    result += L'"';
    return result;
}

int wmain(int argc, wchar_t** argv) {
    const auto lld = environment(L"HUFU_RUST_LLD");
    const auto tools = environment(L"HUFU_GNU_TOOLS_ROOT");
    if (lld.empty() || tools.empty()) { fwprintf(stderr, L"GNU linker environment is missing.\n"); return 2; }
    const wchar_t* triple = nullptr;
    auto args = arguments(argc, argv);
    for (size_t i = 0; i + 1 < args.size(); ++i) {
        if (args[i] == L"-m") {
            if (args[i + 1] == L"i386pep") triple = L"x86_64-w64-mingw32";
            if (args[i + 1] == L"i386pe") triple = L"i686-w64-mingw32";
        }
    }
    if (!triple) { fwprintf(stderr, L"Unsupported GNU PE target.\n"); return 2; }
    for (const auto required : { L"libkernel32.a", L"libmingw32.a", L"libgcc.a", L"libgcc_eh.a", L"libpthread.a" }) {
        const auto path = tools + L"\\mingw\\" + triple + L"\\lib\\" + required;
        if (GetFileAttributesW(path.c_str()) == INVALID_FILE_ATTRIBUTES) { fwprintf(stderr, L"Missing isolated GNU library: %s\nRun prepare-gnu-libraries.py on the pinned Fedora builder or extract its verified bundle.\n", path.c_str()); return 2; }
    }
    std::wstring response;
    for (size_t index = 0; index < args.size(); ++index) {
        auto arg = args[index];
        if (arg == L"-flavor" && index + 1 < args.size()) { ++index; continue; }
        const auto separator = arg.find_last_of(L"/\\");
        const auto basename = separator == std::wstring::npos ? arg : arg.substr(separator + 1);
        if (arg.find(L"self-contained") != std::wstring::npos && (basename == L"crt2.o" || basename == L"dllcrt2.o")) {
            arg = tools + L"\\mingw\\" + triple + L"\\lib\\" + basename;
            if (GetFileAttributesW(arg.c_str()) == INVALID_FILE_ATTRIBUTES) { fwprintf(stderr, L"Missing GNU CRT: %s\n", arg.c_str()); return 2; }
        }
        response += quote(arg) + L"\n";
    }
    wchar_t temporary_directory[MAX_PATH], temporary_file[MAX_PATH];
    if (!GetTempPathW(MAX_PATH, temporary_directory) || !GetTempFileNameW(temporary_directory, L"hgl", 0, temporary_file)) return 2;
    const int byte_count = WideCharToMultiByte(CP_UTF8, 0, response.data(), static_cast<int>(response.size()), nullptr, 0, nullptr, nullptr);
    std::string encoded(byte_count, '\0');
    WideCharToMultiByte(CP_UTF8, 0, response.data(), static_cast<int>(response.size()), encoded.data(), byte_count, nullptr, nullptr);
    { std::ofstream file(temporary_file, std::ios::binary); file.write(encoded.data(), encoded.size()); if (!file) { DeleteFileW(temporary_file); return 2; } }
    // Keep the driver's flavor visible outside the response file. lld's GNU
    // parser consumes the identical -flavor pair again without changing it.
    std::wstring command = quote(lld) + L" -flavor gnu " + quote(std::wstring(L"@") + temporary_file);
    STARTUPINFOW startup{};
    startup.cb = sizeof(startup);
    PROCESS_INFORMATION process{};
    if (!CreateProcessW(lld.c_str(), command.data(), nullptr, nullptr, TRUE, 0, nullptr, nullptr, &startup, &process)) {
        fwprintf(stderr, L"Cannot launch rust-lld: %lu\n", GetLastError()); DeleteFileW(temporary_file); return 2;
    }
    CloseHandle(process.hThread);
    WaitForSingleObject(process.hProcess, INFINITE);
    DWORD code = 2;
    GetExitCodeProcess(process.hProcess, &code);
    CloseHandle(process.hProcess);
    DeleteFileW(temporary_file);
    return static_cast<int>(code);
}
