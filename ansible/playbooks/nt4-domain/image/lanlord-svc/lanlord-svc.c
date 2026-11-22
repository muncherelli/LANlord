/* a tiny win32 service that runs one batch file, the way psexec's psexesvc runs a command.
 * lanlord-exec copies it to ADMIN$\Temp as ll-<id>.exe and creates a service whose image path is
 *   %SystemRoot%\Temp\ll-<id>.exe %SystemRoot%\Temp\ll-<id>.bat
 * it reports running at once, runs cmd.exe /Q /c "<bat>", waits for it, and stops with the exit code
 * of the batch file as the service specific exit code. a stop request kills the child.
 * built for nt4 sp6a x86: no c runtime, only kernel32 and advapi32 calls that exist on nt4.
 * it never reads, prints or handles credentials. */
#include <windows.h>

static SERVICE_STATUS_HANDLE status_handle;
static SERVICE_STATUS status;
static HANDLE volatile child;
static volatile LONG stopping;

static void report(DWORD state, DWORD win32, DWORD specific)
{
    status.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
    status.dwCurrentState = state;
    status.dwControlsAccepted = state == SERVICE_RUNNING ? SERVICE_ACCEPT_STOP : 0;
    status.dwWin32ExitCode = win32;
    status.dwServiceSpecificExitCode = specific;
    status.dwCheckPoint = 0;
    status.dwWaitHint = 0;
    SetServiceStatus(status_handle, &status);
}

static void WINAPI handler(DWORD control)
{
    if (control == SERVICE_CONTROL_STOP) {
        HANDLE h;
        InterlockedExchange((LONG *)&stopping, 1);
        h = child;
        if (h)
            TerminateProcess(h, 1);
        status.dwCurrentState = SERVICE_STOP_PENDING;
        status.dwControlsAccepted = 0;
        status.dwWaitHint = 10000;
    }
    SetServiceStatus(status_handle, &status);
}

/* everything after the program name in the command line */
static const char *args_of(const char *cl)
{
    if (*cl == '"') {
        cl++;
        while (*cl && *cl != '"')
            cl++;
        if (*cl)
            cl++;
    } else {
        while (*cl && *cl != ' ' && *cl != '\t')
            cl++;
    }
    while (*cl == ' ' || *cl == '\t')
        cl++;
    return cl;
}

static void append(char *dst, DWORD *len, DWORD max, const char *s)
{
    while (*s && *len + 1 < max)
        dst[(*len)++] = *s++;
    dst[*len] = 0;
}

static void zero(void *mem, DWORD size)
{
    volatile char *p = (volatile char *)mem;
    while (size--)
        *p++ = 0;
}

static void WINAPI service_main(DWORD argc, LPSTR *argv)
{
    char bat[MAX_PATH + 1], sysdir[MAX_PATH + 1], app[MAX_PATH + 16], cmd[2 * MAX_PATH + 32];
    STARTUPINFOA si;
    PROCESS_INFORMATION pi;
    DWORD len, code = 1, n;
    (void)argc;
    (void)argv;

    status_handle = RegisterServiceCtrlHandlerA("", handler);
    if (!status_handle)
        return;
    report(SERVICE_RUNNING, 0, 0);

    n = ExpandEnvironmentStringsA(args_of(GetCommandLineA()), bat, sizeof bat);
    if (n == 0 || n > sizeof bat || !bat[0] || !GetSystemDirectoryA(sysdir, sizeof sysdir)) {
        report(SERVICE_STOPPED, ERROR_SERVICE_SPECIFIC_ERROR, 1);
        return;
    }
    len = 0;
    app[0] = 0;
    append(app, &len, sizeof app, sysdir);
    append(app, &len, sizeof app, "\\cmd.exe");
    len = 0;
    cmd[0] = 0;
    append(cmd, &len, sizeof cmd, "cmd.exe /Q /c \"");
    append(cmd, &len, sizeof cmd, bat);
    append(cmd, &len, sizeof cmd, "\"");

    zero(&si, sizeof si);
    zero(&pi, sizeof pi);
    si.cb = sizeof si;

    /* no creation flags: the service has no console, so cmd.exe gets its own invisible one */
    if (!CreateProcessA(app, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi)) {
        report(SERVICE_STOPPED, ERROR_SERVICE_SPECIFIC_ERROR, 2);
        return;
    }
    CloseHandle(pi.hThread);
    child = pi.hProcess;
    if (stopping)
        TerminateProcess(pi.hProcess, 1);
    WaitForSingleObject(pi.hProcess, INFINITE);
    if (!GetExitCodeProcess(pi.hProcess, &code))
        code = 1;
    child = NULL;
    CloseHandle(pi.hProcess);
    if (code)
        report(SERVICE_STOPPED, ERROR_SERVICE_SPECIFIC_ERROR, code);
    else
        report(SERVICE_STOPPED, 0, 0);
}

void svc_entry(void)
{
    SERVICE_TABLE_ENTRYA table[2];
    table[0].lpServiceName = "";
    table[0].lpServiceProc = service_main;
    table[1].lpServiceName = NULL;
    table[1].lpServiceProc = NULL;
    StartServiceCtrlDispatcherA(table);
    ExitProcess(0);
}
