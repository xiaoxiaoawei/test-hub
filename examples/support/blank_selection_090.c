#ifndef UNICODE
#define UNICODE
#endif
#ifndef _UNICODE
#define _UNICODE
#endif
#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0A00
#endif
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <wchar.h>

/* Test-only owned fixed text. Never reads document text from another app. */
static const wchar_t source_class[] = L"LingoLeaf.V090.BlankNativeSource";
static const wchar_t source_title[] = L"LingoLeaf 0.9 blank selection source fixture";
static const wchar_t source_text[] =
    L"The warm light filled the room.\r\n\r\n     \r\n\u200B\u200C\u200D\u2060\uFEFF\r\n\r\nAnother visible paragraph.";
static const wchar_t first_sentence[] = L"The warm light filled the room.";
static const wchar_t app_class[] = L"LingoLeaf.NativeWindow";
static const wchar_t chip_title[] = L"轻译";
static const wchar_t inline_title[] = L"轻译 · 划词译文";
enum { EDIT_ID = 101, MOVE_STEPS = 14, POST_UP_MS = 6500, POLL_MS = 10 };

typedef struct {
    HWND window;
    const char *name;
    unsigned show_events;
    unsigned visible_samples;
    unsigned unexpected_visible_samples;
    unsigned hidden_to_visible;
    BOOL initial_visible;
    BOOL may_stay_visible;
    BOOL last_visible;
} Target;

typedef struct {
    DWORD app_pid;
    HANDLE app_process;
    Target targets[2];
    HWINEVENTHOOK app_hook;
    HWINEVENTHOOK calibration_hook;
    HWND calibration_window;
    unsigned calibration_events;
    ULONGLONG started;
    const char *phase;
    BOOL invalid;
    unsigned polls;
    BOOL allow_existing_inline;
} Observer;

static Observer *active_observer = NULL;

static int fail(const char *reason) {
    fprintf(stderr, "{\"kind\":\"error\",\"reason\":\"%s\",\"win32_error\":%lu}\n",
            reason, GetLastError());
    return 1;
}

static HFONT source_font(void) {
    return CreateFontW(-18, 0, 0, 0, FW_NORMAL, FALSE, FALSE, FALSE,
        DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
        DEFAULT_PITCH, L"Segoe UI");
}

static LRESULT CALLBACK source_proc(HWND window, UINT message, WPARAM wp, LPARAM lp) {
    if (message == WM_CLOSE) { DestroyWindow(window); return 0; }
    if (message == WM_DESTROY) { PostQuitMessage(0); return 0; }
    return DefWindowProcW(window, message, wp, lp);
}

static BOOL same_fixture_image(DWORD pid) {
    wchar_t actual[32768] = {0}, own[32768] = {0};
    DWORD length = (DWORD)(sizeof actual / sizeof *actual);
    HANDLE process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
    if (!process) return FALSE;
    BOOL queried = QueryFullProcessImageNameW(process, 0, actual, &length);
    CloseHandle(process);
    DWORD own_length = GetModuleFileNameW(NULL, own, (DWORD)(sizeof own / sizeof *own));
    return queried && own_length > 0 && own_length < (DWORD)(sizeof own / sizeof *own)
        && !_wcsicmp(actual, own);
}

static HWND find_owned_fixture(DWORD *pid, DWORD *thread) {
    HWND window = FindWindowW(source_class, source_title);
    if (!window) return NULL;
    *thread = GetWindowThreadProcessId(window, pid);
    if (!*pid || !*thread || !same_fixture_image(*pid)) {
        SetLastError(ERROR_ACCESS_DENIED);
        return NULL;
    }
    return window;
}

static BOOL send_query(HWND window, UINT message, WPARAM wp, LPARAM lp, DWORD_PTR *result) {
    return SendMessageTimeoutW(window, message, wp, lp,
                              SMTO_ABORTIFHUNG | SMTO_BLOCK, 1000, result) != 0;
}

static BOOL fixture_text_matches(HWND edit) {
    wchar_t text[128] = {0};
    DWORD_PTR count = 0;
    if (!send_query(edit, WM_GETTEXT, sizeof text / sizeof *text, (LPARAM)text, &count)) return FALSE;
    return count == wcslen(source_text) && !wcscmp(text, source_text);
}

static BOOL source_hit(HWND source, HWND edit, POINT point, DWORD pid) {
    HWND hit = WindowFromPoint(point);
    if (hit != edit || GetAncestor(hit, GA_ROOT) != source) return FALSE;
    DWORD actual = 0;
    GetWindowThreadProcessId(hit, &actual);
    return actual == pid;
}

static BOOL move_mouse(POINT point, DWORD extra_flags) {
    int x = GetSystemMetrics(SM_XVIRTUALSCREEN), y = GetSystemMetrics(SM_YVIRTUALSCREEN);
    int width = GetSystemMetrics(SM_CXVIRTUALSCREEN), height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
    if (width <= 1 || height <= 1 || point.x < x || point.y < y
        || point.x >= x + width || point.y >= y + height) return FALSE;
    INPUT input = {0};
    input.type = INPUT_MOUSE;
    input.mi.dx = (LONG)(((int64_t)(point.x - x) * 65535 + (width - 1) / 2) / (width - 1));
    input.mi.dy = (LONG)(((int64_t)(point.y - y) * 65535 + (height - 1) / 2) / (height - 1));
    input.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | extra_flags;
    return SendInput(1, &input, sizeof input) == 1;
}

static void release_mouse_without_move(void) {
    INPUT input = {0};
    input.type = INPUT_MOUSE;
    input.mi.dwFlags = MOUSEEVENTF_LEFTUP;
    SendInput(1, &input, sizeof input);
}

static BOOL char_point(HWND edit, size_t index, POINT *point) {
    DWORD_PTR packed = 0;
    if (!send_query(edit, EM_POSFROMCHAR, (WPARAM)index, 0, &packed)
        || (LONG_PTR)packed == -1) return FALSE;
    point->x = (SHORT)LOWORD(packed);
    point->y = (SHORT)HIWORD(packed);
    return TRUE;
}

static BOOL source_metrics(HWND edit, TEXTMETRICW *metrics, SIZE *last) {
    HDC dc = GetDC(edit);
    if (!dc) return FALSE;
    HFONT face = source_font();
    if (!face) { ReleaseDC(edit, dc); return FALSE; }
    HGDIOBJ previous = SelectObject(dc, face);
    if (!previous || previous == HGDI_ERROR) {
        DeleteObject(face); ReleaseDC(edit, dc); return FALSE;
    }
    BOOL ok = GetTextMetricsW(dc, metrics) && GetTextExtentPoint32W(
        dc, first_sentence + wcslen(first_sentence) - 1, 1, last);
    SelectObject(dc, previous);
    DeleteObject(face);
    ReleaseDC(edit, dc);
    return ok;
}

static BOOL CALLBACK locate_app_windows(HWND window, LPARAM parameter) {
    Observer *observer = (Observer *)parameter;
    DWORD pid = 0;
    GetWindowThreadProcessId(window, &pid);
    if (pid != observer->app_pid) return TRUE;
    wchar_t cls[64] = {0}, title[96] = {0};
    GetClassNameW(window, cls, sizeof cls / sizeof *cls);
    if (wcscmp(cls, app_class)) return TRUE;
    /* Only cached top-level captions of the explicitly supplied app PID. */
    GetWindowTextW(window, title, sizeof title / sizeof *title);
    int index = !wcscmp(title, chip_title) ? 0 : (!wcscmp(title, inline_title) ? 1 : -1);
    if (index >= 0) {
        if (observer->targets[index].window) {
            observer->invalid = TRUE;
        } else {
            observer->targets[index].window = window;
        }
    }
    return TRUE;
}

static void CALLBACK event_proc(HWINEVENTHOOK hook, DWORD event, HWND window,
                               LONG object_id, LONG child_id, DWORD thread, DWORD event_time) {
    Observer *observer = active_observer;
    if (!observer || event != EVENT_OBJECT_SHOW || object_id != OBJID_WINDOW || child_id != CHILDID_SELF)
        return;
    if (hook == observer->calibration_hook && window == observer->calibration_window) {
        ++observer->calibration_events;
        return;
    }
    if (hook != observer->app_hook) return;
    for (int i = 0; i < 2; ++i) {
        Target *target = &observer->targets[i];
        if (window != target->window) continue;
        ++target->show_events;
        printf("{\"kind\":\"unexpected_app_show\",\"target\":\"%s\",\"window\":\"%p\",\"phase\":\"%s\",\"elapsed_ms\":%llu,\"event_time\":%lu,\"event_thread\":%lu,\"visible_now\":%d}\n",
               target->name, (void *)window, observer->phase,
               (unsigned long long)(GetTickCount64() - observer->started),
               event_time, thread, IsWindowVisible(window) != 0);
        fflush(stdout);
    }
}

static void observer_pump(Observer *observer) {
    MSG message = {0};
    while (PeekMessageW(&message, NULL, 0, 0, PM_REMOVE)) {
        if (message.message != WM_QUIT) {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    if (!observer) return;
    ++observer->polls;
    DWORD code = 0;
    if (!GetExitCodeProcess(observer->app_process, &code) || code != STILL_ACTIVE) {
        if (!observer->invalid) fail("The explicitly observed app exited during the watch");
        observer->invalid = TRUE;
    }
    for (int i = 0; i < 2; ++i) {
        Target *target = &observer->targets[i];
        DWORD pid = 0;
        GetWindowThreadProcessId(target->window, &pid);
        if (!IsWindow(target->window) || pid != observer->app_pid) {
            observer->invalid = TRUE;
            continue;
        }
        BOOL visible = IsWindowVisible(target->window) != 0;
        if (visible) {
            ++target->visible_samples;
            if (!target->may_stay_visible) ++target->unexpected_visible_samples;
            if (!target->last_visible) {
                ++target->hidden_to_visible;
                printf("{\"kind\":\"unexpected_app_visible\",\"target\":\"%s\",\"window\":\"%p\",\"phase\":\"%s\",\"elapsed_ms\":%llu}\n",
                       target->name, (void *)target->window, observer->phase,
                       (unsigned long long)(GetTickCount64() - observer->started));
                fflush(stdout);
            }
        } else {
            if (target->last_visible) {
                printf("{\"kind\":\"observed_app_hidden\",\"target\":\"%s\",\"window\":\"%p\",\"phase\":\"%s\",\"elapsed_ms\":%llu}\n",
                       target->name, (void *)target->window, observer->phase,
                       (unsigned long long)(GetTickCount64() - observer->started));
                fflush(stdout);
            }
            /* Initial pin visibility is only allowed continuously until hidden. */
            target->may_stay_visible = FALSE;
        }
        target->last_visible = visible;
    }
}

static void watched_wait(Observer *observer, DWORD milliseconds) {
    ULONGLONG until = GetTickCount64() + milliseconds;
    do {
        observer_pump(observer);
        ULONGLONG now = GetTickCount64();
        if (now >= until) break;
        ULONGLONG left = until - now;
        Sleep((DWORD)(left < POLL_MS ? left : POLL_MS));
    } while (TRUE);
    observer_pump(observer);
}

static BOOL observer_healthy(const Observer *observer) {
    return !observer->invalid && observer->calibration_events != 0
        && observer->targets[0].show_events == 0 && observer->targets[1].show_events == 0
        && observer->targets[0].unexpected_visible_samples == 0
        && observer->targets[1].unexpected_visible_samples == 0
        && observer->targets[0].hidden_to_visible == 0
        && observer->targets[1].hidden_to_visible == 0;
}

static void observer_stop(Observer *observer) {
    active_observer = NULL;
    if (observer->app_hook) UnhookWinEvent(observer->app_hook);
    if (observer->calibration_hook) UnhookWinEvent(observer->calibration_hook);
    if (observer->calibration_window) DestroyWindow(observer->calibration_window);
    if (observer->app_process) CloseHandle(observer->app_process);
    observer->app_hook = NULL;
    observer->calibration_hook = NULL;
    observer->app_process = NULL;
    observer->calibration_window = NULL;
}

static BOOL observer_start(Observer *observer, DWORD app_pid, BOOL allow_existing_inline) {
    ZeroMemory(observer, sizeof *observer);
    observer->app_pid = app_pid;
    observer->allow_existing_inline = allow_existing_inline;
    observer->phase = "arming";
    observer->started = GetTickCount64();
    observer->targets[0].name = "chip";
    observer->targets[1].name = "inline";
    observer->app_process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, FALSE, app_pid);
    if (!observer->app_process) { fail("Cannot bind the explicit observed app process"); return FALSE; }
    EnumWindows(locate_app_windows, (LPARAM)observer);
    if (observer->invalid || !observer->targets[0].window || !observer->targets[1].window) {
        fail("Expected exactly one known chip and inline in the explicit LingoLeaf app PID");
        observer_stop(observer); return FALSE;
    }
    MSG message = {0};
    PeekMessageW(&message, NULL, 0, 0, PM_NOREMOVE);
    active_observer = observer;
    observer->app_hook = SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, NULL, event_proc,
                                       app_pid, 0, WINEVENT_OUTOFCONTEXT);
    observer->calibration_hook = SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, NULL, event_proc,
                                               GetCurrentProcessId(), 0, WINEVENT_OUTOFCONTEXT);
    if (!observer->app_hook || !observer->calibration_hook) {
        fail("Cannot install actual out-of-context SHOW event hooks");
        observer_stop(observer); return FALSE;
    }
    for (int i = 0; i < 2; ++i) {
        Target *target = &observer->targets[i];
        target->initial_visible = IsWindowVisible(target->window) != 0;
        target->last_visible = target->initial_visible;
        target->may_stay_visible = allow_existing_inline && i == 1 && target->initial_visible;
    }
    printf("{\"kind\":\"observer_initial_visibility\",\"app_pid\":%lu,\"chip_visible\":%d,\"inline_visible\":%d,\"allow_existing_inline\":%d}\n",
           app_pid, observer->targets[0].initial_visible,
           observer->targets[1].initial_visible, allow_existing_inline);
    fflush(stdout);
    observer_pump(observer);
    observer->phase = "native_self_calibration";
    observer->calibration_window = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        L"STATIC", L"", WS_POPUP, 0, 0, 1, 1, NULL, NULL, GetModuleHandleW(NULL), NULL);
    if (!observer->calibration_window) {
        fail("Cannot create the helper-only native SHOW calibration window");
        observer_stop(observer); return FALSE;
    }
    ShowWindow(observer->calibration_window, SW_SHOWNOACTIVATE);
    watched_wait(observer, 180);
    ShowWindow(observer->calibration_window, SW_HIDE);
    watched_wait(observer, 30);
    printf("{\"kind\":\"observer_calibration\",\"native_show_callbacks\":%u,\"calibration_own_pid\":%lu,\"app_pid\":%lu,\"passed\":%d}\n",
           observer->calibration_events, GetCurrentProcessId(), app_pid,
           observer->calibration_events != 0);
    fflush(stdout);
    if (!observer_healthy(observer)) {
        fail("SHOW calibration failed or app popup was already shown during arming");
        observer_stop(observer); return FALSE;
    }
    observer->phase = "armed";
    printf("{\"kind\":\"observer_armed\",\"app_pid\":%lu,\"chip\":\"%p\",\"inline\":\"%p\",\"nominal_poll_ms\":%d,\"event\":\"EVENT_OBJECT_SHOW OBJID_WINDOW\"}\n",
           app_pid, (void *)observer->targets[0].window, (void *)observer->targets[1].window, POLL_MS);
    fflush(stdout);
    return TRUE;
}

static int observer_summary(Observer *observer, const char *gesture, BOOL selection_valid,
                            BOOL source_collapsed, unsigned selected_start,
                            unsigned selected_end, ULONGLONG up_time) {
    observer_pump(observer);
    BOOL selection_checked = up_time != 0;
    BOOL pass = observer_healthy(observer) && (!selection_checked || selection_valid);
    ULONGLONG now = GetTickCount64();
    printf("{\"kind\":\"blank_popup_watch_result\",\"gesture\":\"%s\",\"app_pid\":%lu,\"passed\":%d,\"source_selection_checked\":%d,\"source_selection_valid\":%d,\"source_collapsed\":%d,\"selected_start\":%u,\"selected_end\":%u,\"allow_existing_inline\":%d,\"chip_initial_visible\":%d,\"inline_initial_visible\":%d,\"chip_final_visible\":%d,\"inline_final_visible\":%d,\"chip_show_events\":%u,\"inline_show_events\":%u,\"chip_visible_polls\":%u,\"inline_visible_polls\":%u,\"chip_unexpected_visible_polls\":%u,\"inline_unexpected_visible_polls\":%u,\"chip_hidden_to_visible\":%u,\"inline_hidden_to_visible\":%u,\"polls\":%u,\"calibration_callbacks\":%u,\"elapsed_ms\":%llu,\"observed_after_up_ms\":%llu,\"controller_messages_sent\":0}\n",
           gesture, observer->app_pid, pass, selection_checked,
           selection_checked && selection_valid, source_collapsed, selected_start, selected_end,
           observer->allow_existing_inline,
           observer->targets[0].initial_visible, observer->targets[1].initial_visible,
           observer->targets[0].last_visible, observer->targets[1].last_visible,
           observer->targets[0].show_events, observer->targets[1].show_events,
           observer->targets[0].visible_samples, observer->targets[1].visible_samples,
           observer->targets[0].unexpected_visible_samples, observer->targets[1].unexpected_visible_samples,
           observer->targets[0].hidden_to_visible, observer->targets[1].hidden_to_visible,
           observer->polls, observer->calibration_events,
           (unsigned long long)(now - observer->started),
           (unsigned long long)(up_time ? now - up_time : 0));
    fflush(stdout);
    return pass ? 0 : 1;
}

static int prepare_source(HWND *window, HWND *edit, DWORD *pid, DWORD *thread) {
    *window = find_owned_fixture(pid, thread);
    if (!*window) return fail("The exact owned native source fixture was not found");
    *edit = GetDlgItem(*window, EDIT_ID);
    wchar_t cls[32] = {0};
    GetClassNameW(*edit, cls, sizeof cls / sizeof *cls);
    DWORD child_pid = 0;
    GetWindowThreadProcessId(*edit, &child_pid);
    if (!*edit || child_pid != *pid || _wcsicmp(cls, L"EDIT") || !fixture_text_matches(*edit))
        return fail("The owned fixed readonly source EDIT identity or text did not match");
    if ((GetWindowLongPtrW(*edit, GWL_STYLE) & ES_READONLY) == 0)
        return fail("Source EDIT unexpectedly became writable");
    MSG queue = {0};
    PeekMessageW(&queue, NULL, 0, 0, PM_NOREMOVE);
    DWORD caller = GetCurrentThreadId();
    if (!AttachThreadInput(caller, *thread, TRUE)) return fail("Cannot attach to the owned source input queue");
    SetForegroundWindow(*window);
    SetFocus(*edit);
    AttachThreadInput(caller, *thread, FALSE);
    ULONGLONG until = GetTickCount64() + 750;
    while (GetForegroundWindow() != *window && GetTickCount64() < until) {
        SetForegroundWindow(*window);
        Sleep(10);
    }
    GUITHREADINFO info = {0};
    info.cbSize = sizeof info;
    if (GetForegroundWindow() != *window || !GetGUIThreadInfo(*thread, &info) || info.hwndFocus != *edit)
        return fail("Owned readonly source EDIT did not receive foreground keyboard focus");
    if ((GetAsyncKeyState(VK_LBUTTON) & 0x8000) || (GetAsyncKeyState(VK_RBUTTON) & 0x8000)
        || (GetAsyncKeyState(VK_SHIFT) & 0x8000) || (GetAsyncKeyState(VK_CONTROL) & 0x8000)
        || (GetAsyncKeyState(VK_MENU) & 0x8000))
        return fail("A physical pointer button or modifier was already held");
    return 0;
}

static int run_drag(const char *mode, DWORD app_pid, DWORD observe_ms, BOOL allow_existing_inline) {
    HWND window = NULL, edit = NULL;
    DWORD source_pid = 0, source_thread = 0;
    if (prepare_source(&window, &edit, &source_pid, &source_thread)) return 1;
    TEXTMETRICW metrics = {0};
    SIZE last = {0};
    if (!source_metrics(edit, &metrics, &last)) return fail("Cannot measure the fixed owned source text");
    size_t length = wcslen(first_sentence);
    POINT begin = {0}, end = {0};
    unsigned expected_caret = 0;
    BOOL blank = strcmp(mode, "drag") != 0;
    if (!strcmp(mode, "blank-tail")) {
        if (!char_point(edit, length - 1, &begin)) return fail("Cannot locate the first-line tail");
        begin.x += last.cx + 40;
        begin.y += metrics.tmHeight / 2;
        end = begin;
        end.x += 140;
        expected_caret = (unsigned)length;
    } else if (!strcmp(mode, "blank-line")) {
        DWORD_PTR start = 0;
        if (!send_query(edit, EM_LINEINDEX, 1, 0, &start) || (LONG_PTR)start < 0
            || !char_point(edit, (size_t)start, &begin))
            return fail("Cannot locate the actual empty second line");
        if (start != length + 2) return fail("The second line was not the intended fixed empty line");
        begin.x += 40;
        begin.y += metrics.tmHeight / 2;
        end = begin;
        end.x += 140;
        expected_caret = (unsigned)start;
    } else if (!strcmp(mode, "blank-spaces")) {
        DWORD_PTR start = 0;
        if (!send_query(edit, EM_LINEINDEX, 2, 0, &start) || start != length + 4
            || !char_point(edit, (size_t)start, &begin)
            || !char_point(edit, (size_t)start + 5, &end))
            return fail("Cannot locate the actual five-space third line");
        begin.x += 1;
        begin.y += metrics.tmHeight / 2;
        end.x += 2;
        end.y += metrics.tmHeight / 2;
        if (end.x - begin.x < 12 || begin.y != end.y)
            return fail("Five-space gesture is too short or unexpectedly wrapped");
        expected_caret = (unsigned)start;
    } else if (!strcmp(mode, "blank-invisible")) {
        DWORD_PTR invisible_row = 0, empty_row = 0;
        if (!send_query(edit, EM_LINEINDEX, 3, 0, &invisible_row) || invisible_row != 42
            || !send_query(edit, EM_LINEINDEX, 4, 0, &empty_row) || empty_row != 49
            || !char_point(edit, 40, &begin) || !char_point(edit, 49, &end))
            return fail("Cannot locate the fixed zero-width range and following empty row");
        begin.x += 40;
        begin.y += metrics.tmHeight / 2;
        end.x += 40;
        end.y += metrics.tmHeight / 2;
        if (end.y - begin.y < metrics.tmHeight)
            return fail("The invisible-range gesture did not cross the intended source rows");
        expected_caret = 40;
    } else {
        if (!char_point(edit, 0, &begin) || !char_point(edit, length - 1, &end))
            return fail("Cannot locate fixed first-sentence characters");
        begin.x += 1;
        begin.y += metrics.tmHeight / 2;
        end.x += last.cx + 2;
        end.y += metrics.tmHeight / 2;
    }
    RECT client = {0};
    GetClientRect(edit, &client);
    if (begin.x < 0 || begin.y < 0 || end.x >= client.right || end.y >= client.bottom)
        return fail("The fixed gesture path does not fit the owned source client");
    if (!ClientToScreen(edit, &begin) || !ClientToScreen(edit, &end))
        return fail("Cannot map owned gesture points to screen coordinates");
    for (int i = 0; i <= MOVE_STEPS; ++i) {
        POINT at = {begin.x + (end.x - begin.x) * i / MOVE_STEPS,
                    begin.y + (end.y - begin.y) * i / MOVE_STEPS};
        if (!source_hit(window, edit, at, source_pid))
            return fail("A fixed drag path point is obscured; no mouse input was sent");
    }
    Observer observer = {0};
    Observer *watch = NULL;
    if (blank) {
        if (!observer_start(&observer, app_pid, allow_existing_inline)) return 1;
        watch = &observer;
    }
    DWORD_PTR initial = 0;
    if (!send_query(edit, EM_GETSEL, 0, 0, &initial)) {
        if (watch) observer_stop(watch);
        return fail("Cannot inspect only the owned source initial selection");
    }
    if (watch) watch->phase = "pointer_positioning";
    if (!move_mouse(begin, 0)) {
        if (watch) observer_stop(watch);
        return fail("Cannot move to the exact visible owned gesture start");
    }
    watched_wait(watch, 40);
    if (!source_hit(window, edit, begin, source_pid)) {
        if (watch) observer_stop(watch);
        return fail("Source became obscured before owned mouse down");
    }
    if (watch) watch->phase = "gesture_down";
    if (!move_mouse(begin, MOUSEEVENTF_LEFTDOWN)) {
        if (watch) observer_stop(watch);
        return fail("Cannot press on the verified owned source");
    }
    BOOL input_ok = TRUE;
    for (int i = 1; i <= MOVE_STEPS; ++i) {
        if (watch) watch->phase = "gesture_moves";
        POINT at = {begin.x + (end.x - begin.x) * i / MOVE_STEPS,
                    begin.y + (end.y - begin.y) * i / MOVE_STEPS};
        if (!source_hit(window, edit, at, source_pid) || !move_mouse(at, 0)) {
            input_ok = FALSE;
            break;
        }
        watched_wait(watch, 16);
    }
    if (watch) watch->phase = "gesture_up";
    if (input_ok && source_hit(window, edit, end, source_pid)) {
        input_ok = move_mouse(end, MOUSEEVENTF_LEFTUP);
        if (!input_ok) release_mouse_without_move();
    } else {
        release_mouse_without_move();
        input_ok = FALSE;
    }
    ULONGLONG up_time = GetTickCount64();
    if (watch) watch->phase = "post_up";
    watched_wait(watch, blank ? observe_ms : 500);
    DWORD_PTR selected = 0;
    BOOL queried = send_query(edit, EM_GETSEL, 0, 0, &selected);
    unsigned start = (unsigned)LOWORD(selected), finish = (unsigned)HIWORD(selected);
    BOOL collapsed = queried && start == finish;
    BOOL spaces_only = queried && start < finish && finish <= wcslen(source_text);
    if (spaces_only) {
        for (unsigned at = start; at < finish; ++at) {
            if (source_text[at] != L' ') { spaces_only = FALSE; break; }
        }
    }
    BOOL invisible_only = queried && start < finish && finish <= wcslen(source_text);
    if (invisible_only) {
        for (unsigned at = start; at < finish; ++at) {
            wchar_t value = source_text[at];
            if (value != L'\r' && value != L'\n' && value != 0x200b
                && value != 0x200c && value != 0x200d && value != 0x2060 && value != 0xfeff) {
                invisible_only = FALSE;
                break;
            }
        }
    }
    BOOL exact = queried && (blank ? collapsed && start == expected_caret : start == 0 && finish == length);
    if (!strcmp(mode, "blank-spaces"))
        exact = queried && !collapsed && spaces_only && start == expected_caret && finish == expected_caret + 5;
    if (!strcmp(mode, "blank-invisible"))
        exact = queried && !collapsed && invisible_only && start == 40 && finish == 49;
    if (!fixture_text_matches(edit)) exact = FALSE;
    printf("{\"kind\":\"actual_owned_source_gesture\",\"gesture\":\"%s\",\"source_pid\":%lu,\"source\":\"%p\",\"edit\":\"%p\",\"begin\":[%ld,%ld],\"end\":[%ld,%ld],\"move_steps\":%d,\"initial_start\":%u,\"initial_end\":%u,\"selected_start\":%u,\"selected_end\":%u,\"expected_caret\":%u,\"input_ok\":%d,\"exact_selection\":%d,\"source_collapsed\":%d,\"selected_only_ascii_spaces\":%d,\"selected_only_invisible_or_linebreaks\":%d,\"controller_messages_sent\":0,\"selection_messages_sent\":0,\"input\":\"real SendInput down/14moves/up\"}\n",
           mode, source_pid, (void *)window, (void *)edit, begin.x, begin.y, end.x, end.y, MOVE_STEPS,
           (unsigned)LOWORD(initial), (unsigned)HIWORD(initial), start, finish, expected_caret,
           input_ok, exact, collapsed, spaces_only, invisible_only);
    fflush(stdout);
    int result = input_ok && exact ? 0 : 1;
    if (watch) {
        result |= observer_summary(watch, mode, exact, collapsed, start, finish, up_time);
        observer_stop(watch);
    }
    if (result) fail("Real owned gesture, expected source selection, or no-popup observation failed");
    return result;
}

static int run_watch(DWORD app_pid, DWORD duration, BOOL allow_existing_inline) {
    DWORD source_pid = 0, source_thread = 0;
    if (!find_owned_fixture(&source_pid, &source_thread))
        return fail("Standalone watch requires the exact owned source fixture to be running");
    Observer observer = {0};
    if (!observer_start(&observer, app_pid, allow_existing_inline)) return 1;
    observer.phase = "standalone_watch";
    watched_wait(&observer, duration);
    int result = observer_summary(&observer, "watch_only", TRUE, FALSE, 0, 0, 0);
    observer_stop(&observer);
    return result;
}

static int run_source(void) {
    if (FindWindowW(source_class, source_title)) return fail("The exact source fixture is already running");
    HINSTANCE instance = GetModuleHandleW(NULL);
    WNDCLASSW cls = {0};
    cls.lpfnWndProc = source_proc;
    cls.hInstance = instance;
    cls.hCursor = LoadCursorW(NULL, IDC_IBEAM);
    cls.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
    cls.lpszClassName = source_class;
    if (!RegisterClassW(&cls)) return fail("Cannot register the unique owned source class");
    HWND window = CreateWindowExW(0, source_class, source_title,
        WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN, 60, 70, 780, 300,
        NULL, NULL, instance, NULL);
    if (!window) return fail("Cannot create the owned source window");
    HWND edit = CreateWindowExW(0, L"EDIT", source_text,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_MULTILINE | ES_READONLY,
        24, 42, 700, 150, window, (HMENU)(INT_PTR)EDIT_ID, instance, NULL);
    HFONT face = source_font();
    if (!edit || !face) {
        DestroyWindow(window);
        if (face) DeleteObject(face);
        return fail("Cannot create fixed readonly source EDIT/font");
    }
    SendMessageW(edit, WM_SETFONT, (WPARAM)face, TRUE);
    ShowWindow(window, SW_SHOW);
    UpdateWindow(window);
    SetForegroundWindow(window);
    SetFocus(edit);
    printf("{\"kind\":\"owned_blank_source_ready\",\"pid\":%lu,\"window\":\"%p\",\"edit\":\"%p\",\"font_pixels\":18,\"first_sentence_chars\":%zu,\"fixed_text_chars\":%zu,\"readonly\":true,\"paragraph_gap\":\"empty row1, five spaces row2, five invisible format chars row3, empty row4\"}\n",
           GetCurrentProcessId(), (void *)window, (void *)edit,
           wcslen(first_sentence), wcslen(source_text));
    fflush(stdout);
    MSG message = {0};
    while (GetMessageW(&message, NULL, 0, 0) > 0) {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
    DeleteObject(face);
    return 0;
}

static int close_source(void) {
    DWORD pid = 0, thread = 0;
    HWND window = find_owned_fixture(&pid, &thread);
    if (!window) return fail("The exact owned source fixture is not running");
    if (!PostMessageW(window, WM_CLOSE, 0, 0)) return fail("Cannot close only the owned source fixture");
    ULONGLONG until = GetTickCount64() + 2000;
    while (IsWindow(window) && GetTickCount64() < until) Sleep(10);
    printf("{\"kind\":\"owned_blank_source_close\",\"pid\":%lu,\"closed\":%d}\n", pid, !IsWindow(window));
    return IsWindow(window) ? 1 : 0;
}

static BOOL parse_dword(const wchar_t *value, DWORD *result) {
    for (const wchar_t *at = value; *at; ++at) {
        if (*at < L'0' || *at > L'9') return FALSE;
    }
    wchar_t *end = NULL;
    errno = 0;
    unsigned long number = wcstoul(value, &end, 10);
    if (errno || !value[0] || !end || *end || !number || number > 0xffffffffUL) return FALSE;
    *result = (DWORD)number;
    return TRUE;
}

int wmain(int argc, wchar_t **argv) {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    if (argc == 2 && !wcscmp(argv[1], L"source")) return run_source();
    if (argc == 2 && !wcscmp(argv[1], L"close")) return close_source();
    if (argc == 2 && !wcscmp(argv[1], L"drag")) return run_drag("drag", 0, 0, FALSE);
    DWORD pid = 0;
    if (argc >= 3 && argc <= 5 && parse_dword(argv[2], &pid)) {
        BOOL allow = !wcscmp(argv[argc - 1], L"--allow-existing-inline");
        int values = argc - (allow ? 1 : 0);
        DWORD duration = POST_UP_MS;
        BOOL valid_duration = values == 3 || (values == 4 && parse_dword(argv[3], &duration));
        if (valid_duration) {
            if (duration <= 15000) {
                if (!wcscmp(argv[1], L"blank-tail")) return run_drag("blank-tail", pid, duration, allow);
                if (!wcscmp(argv[1], L"blank-line")) return run_drag("blank-line", pid, duration, allow);
                if (!wcscmp(argv[1], L"blank-spaces")) return run_drag("blank-spaces", pid, duration, allow);
                if (!wcscmp(argv[1], L"blank-invisible")) return run_drag("blank-invisible", pid, duration, allow);
            }
            if (duration <= 30000 && !wcscmp(argv[1], L"watch")) return run_watch(pid, duration, allow);
        }
    }
    fputs("Usage: lingoleaf-v090-blank-source.exe source|close|drag|blank-tail APP_PID [1..15000 ms] [--allow-existing-inline]|blank-line APP_PID [ms] [flag]|blank-spaces APP_PID [ms] [flag]|blank-invisible APP_PID [ms] [flag]|watch APP_PID [1..30000 ms] [flag]; default observe 6500ms\n", stderr);
    return 2;
}
