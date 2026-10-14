#pragma once

#include <windows.h>
#include <algorithm>
#include <stdexcept>
#include <streambuf>
#include <string>

namespace mixyard {
struct OutputError : std::runtime_error {
    std::string code;
    explicit OutputError(DWORD error)
        : std::runtime_error("Cannot exclusively create output, Win32=" + std::to_string(error)),
          code(error == ERROR_FILE_EXISTS || error == ERROR_ALREADY_EXISTS ? "output-exists" : "output-unavailable") {}
};

// Writer-only stream adapter. The acquired handle is never closed and reopened.
class ExclusiveFileBuffer final : public std::streambuf {
    HANDLE file_ = INVALID_HANDLE_VALUE;
protected:
    std::streamsize xsputn(const char* data, std::streamsize count) override {
        std::streamsize total = 0;
        while (total < count) {
            const auto chunk = static_cast<DWORD>(std::min<std::streamsize>(count - total, MAXDWORD));
            DWORD written = 0;
            if (!WriteFile(file_, data + total, chunk, &written, nullptr) || !written) break;
            total += written;
        }
        return total;
    }
    int_type overflow(int_type value) override {
        if (traits_type::eq_int_type(value, traits_type::eof())) return traits_type::not_eof(value);
        const char byte = traits_type::to_char_type(value);
        return xsputn(&byte, 1) == 1 ? value : traits_type::eof();
    }
    pos_type seekoff(off_type offset, std::ios_base::seekdir direction, std::ios_base::openmode mode) override {
        if (!(mode & std::ios_base::out)) return pos_type(off_type(-1));
        const DWORD origin = direction == std::ios_base::beg ? FILE_BEGIN :
                             direction == std::ios_base::cur ? FILE_CURRENT : FILE_END;
        LARGE_INTEGER distance{}, position{}; distance.QuadPart = offset;
        if (!SetFilePointerEx(file_, distance, &position, origin)) return pos_type(off_type(-1));
        return pos_type(position.QuadPart);
    }
    pos_type seekpos(pos_type position, std::ios_base::openmode mode) override {
        return seekoff(static_cast<off_type>(position), std::ios_base::beg, mode);
    }
    int sync() override { return FlushFileBuffers(file_) ? 0 : -1; }
public:
    explicit ExclusiveFileBuffer(const std::wstring& path) {
        file_ = CreateFileW(path.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr);
        if (file_ == INVALID_HANDLE_VALUE) throw OutputError(GetLastError());
    }
    ~ExclusiveFileBuffer() override { if (file_ != INVALID_HANDLE_VALUE) CloseHandle(file_); }
    ExclusiveFileBuffer(const ExclusiveFileBuffer&) = delete;
    ExclusiveFileBuffer& operator=(const ExclusiveFileBuffer&) = delete;
    bool close() {
        if (file_ == INVALID_HANDLE_VALUE) return true;
        const bool closed = CloseHandle(file_) != 0;
        if (closed) file_ = INVALID_HANDLE_VALUE;
        return closed;
    }
};
} // namespace mixyard
