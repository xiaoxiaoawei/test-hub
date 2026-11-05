// Public, credential-free update smoke fixture. Build both ACK_STARTUP=1 and 0.
// This never creates a window or connects to the network.
#include <windows.h>
#include <shellapi.h>
#include <stdio.h>
#include <string.h>
#include <wchar.h>

#ifndef ACK_STARTUP
#define ACK_STARTUP 1
#endif

static int write_marker(const wchar_t *name, const char *value) {
    wchar_t path[32768];
    DWORD length = GetEnvironmentVariableW(L"LOCALAPPDATA", path, 32768);
    if (!length || length >= 32768 || length + wcslen(name) + 2 >= 32768)
        return 0;
    path[length++] = L'\\';
    wcscpy(path + length, name);
    HANDLE file = CreateFileW(path, GENERIC_WRITE, FILE_SHARE_READ, NULL,
                              CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (file == INVALID_HANDLE_VALUE)
        return 0;
    DWORD written = 0;
    BOOL result = WriteFile(file, value, (DWORD)strlen(value), &written, NULL);
    FlushFileBuffers(file);
    CloseHandle(file);
    return result && written == strlen(value);
}

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE previous, LPWSTR command, int show) {
    (void)instance;
    (void)previous;
    (void)command;
    (void)show;
    int count = 0;
    wchar_t **args = CommandLineToArgvW(GetCommandLineW(), &count);
    if (!args || count != 3 || wcscmp(args[1], L"--lingoleaf-update-restarted")) {
        if (args) LocalFree(args);
        return 42;
    }
    char pid[32];
    snprintf(pid, sizeof(pid), "%lu\n", GetCurrentProcessId());
    if (!write_marker(L"candidate.pid", pid)) {
        LocalFree(args);
        return 44;
    }
#if ACK_STARTUP
    if (wcslen(args[2]) != 32) {
        LocalFree(args);
        return 45;
    }
    wchar_t event_name[128] = L"Local\\LingoLeaf.Update.AppReady.";
    wcscat(event_name, args[2]);
    HANDLE ready = OpenEventW(EVENT_MODIFY_STATE, FALSE, event_name);
    LocalFree(args);
    if (!ready || !SetEvent(ready)) {
        if (ready) CloseHandle(ready);
        return 46;
    }
    CloseHandle(ready);
    if (!write_marker(L"candidate-ready.txt", "startup acknowledged\n"))
        return 47;
    // A real application stays resident after acknowledging startup. Keep the
    // fixture alive while the helper confirms the process and removes backup.
    Sleep(5000);
    return 0;
#else
    LocalFree(args);
    write_marker(L"candidate-failed.txt", "deliberate startup failure\n");
    return 43;
#endif
}
