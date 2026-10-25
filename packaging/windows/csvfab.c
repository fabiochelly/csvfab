/* csvfab.exe: the Windows entry point of the installed app.
   winget refuses any package whose entry point is a script (.cmd, .ps1…), so the
   installer ships this native launcher instead of csvfab.cmd: it runs csvfab.py
   with the Python bundled beside it (python\pythonw.exe, the embeddable
   distribution) and returns at once — csvfab.py detaches the server and the
   browser itself. The command line is handed over untouched, quotes included, and
   the working directory is the caller's, so "csvfab x.csv" in a terminal still
   resolves x.csv where it was typed.
   Built by .github/workflows/release.yml (cl /MT, subsystem windows: no console). */
#include <windows.h>
#include <stdio.h>
#include <wchar.h>

int WINAPI wWinMain(HINSTANCE inst, HINSTANCE prev, PWSTR args, int show)
{
    WCHAR dir[4 * MAX_PATH], exe[4 * MAX_PATH + 32];
    DWORD n = GetModuleFileNameW(NULL, dir, ARRAYSIZE(dir));
    if (n == 0 || n == ARRAYSIZE(dir))
        return 1;
    while (n && dir[n - 1] != L'\\')
        n--;
    dir[n] = 0;                                   /* keeps the trailing backslash */
    swprintf(exe, ARRAYSIZE(exe), L"%lspython\\pythonw.exe", dir);

    size_t len = wcslen(exe) + wcslen(dir) + wcslen(args) + 32;
    WCHAR *cmd = HeapAlloc(GetProcessHeap(), 0, len * sizeof(WCHAR));
    if (!cmd)
        return 1;
    swprintf(cmd, len, L"\"%ls\" \"%lscsvfab.py\" %ls", exe, dir, args);

    STARTUPINFOW si = { 0 };
    si.cb = sizeof si;
    PROCESS_INFORMATION pi;
    if (!CreateProcessW(exe, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi)) {
        WCHAR msg[6 * MAX_PATH];
        swprintf(msg, ARRAYSIZE(msg), L"csvfab could not start its bundled Python:\n%ls\n\n"
                 L"Reinstall csvfab (winget install FabioChelly.csvfab).", exe);
        MessageBoxW(NULL, msg, L"csvfab", MB_ICONERROR);
        return 1;
    }
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    return 0;
}
