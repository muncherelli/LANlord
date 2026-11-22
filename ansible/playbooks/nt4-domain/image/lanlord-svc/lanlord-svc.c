/* a tiny win32 service that runs one batch file, the way psexec's psexesvc runs a command.
 * lanlord-exec copies it to ADMIN$\Temp as ll-<id>.exe and creates a service whose image path is
 *   %SystemRoot%\Temp\ll-<id>.exe %SystemRoot%\Temp\ll-<id>.bat
 * it reports running at once, runs cmd.exe /Q /c "<bat>", waits for it, and stops with the exit code
 * of the batch file as the service specific exit code. a stop request kills the child.
 * built for nt4 sp6a x86: no c runtime, only kernel32 and advapi32 calls that exist on nt4.
 * it never reads, prints or handles credentials. */
/* the few win32 declarations needed, written out so the build needs no windows headers (and so it
 * runs on any build host architecture); every function is stdcall and exists on nt4 */
typedef unsigned long DWORD;
typedef int BOOL;
typedef void *HANDLE;
typedef char *LPSTR;
typedef const char *LPCSTR;
#define WINAPI __attribute__((stdcall))
#define DECL(type) __declspec(dllimport) type WINAPI
#define NULL ((void *)0)
#define TRUE 1
#define FALSE 0
#define INFINITE 0xFFFFFFFFUL
#define MAX_PATH 260
#define SERVICE_WIN32_OWN_PROCESS 0x10
#define SERVICE_STOPPED 1
#define SERVICE_STOP_PENDING 3
#define SERVICE_RUNNING 4
#define SERVICE_ACCEPT_STOP 1
#define SERVICE_CONTROL_STOP 1
#define ERROR_SERVICE_SPECIFIC_ERROR 1066

typedef struct {
    DWORD dwServiceType, dwCurrentState, dwControlsAccepted, dwWin32ExitCode;
    DWORD dwServiceSpecificExitCode, dwCheckPoint, dwWaitHint;
} SERVICE_STATUS;
typedef void *SERVICE_STATUS_HANDLE;
typedef void(WINAPI *LPHANDLER_FUNCTION)(DWORD);
typedef void(WINAPI *LPSERVICE_MAIN_FUNCTIONA)(DWORD, LPSTR *);
typedef struct {
    LPCSTR lpServiceName;
    LPSERVICE_MAIN_FUNCTIONA lpServiceProc;
} SERVICE_TABLE_ENTRYA;
typedef struct {
    DWORD cb;
    LPSTR lpReserved, lpDesktop, lpTitle;
    DWORD dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
    unsigned short wShowWindow, cbReserved2;
    unsigned char *lpReserved2;
    HANDLE hStdInput, hStdOutput, hStdError;
} STARTUPINFOA;
typedef struct {
    HANDLE hProcess, hThread;
    DWORD dwProcessId, dwThreadId;
} PROCESS_INFORMATION;
/* the real structure is 24 bytes on x86 */
typedef struct {
    void *reserved[6];
} CRITICAL_SECTION;

DECL(BOOL) StartServiceCtrlDispatcherA(const SERVICE_TABLE_ENTRYA *);
DECL(SERVICE_STATUS_HANDLE) RegisterServiceCtrlHandlerA(LPCSTR, LPHANDLER_FUNCTION);
DECL(BOOL) SetServiceStatus(SERVICE_STATUS_HANDLE, SERVICE_STATUS *);
DECL(void) ExitProcess(unsigned int);
DECL(DWORD) ExpandEnvironmentStringsA(LPCSTR, LPSTR, DWORD);
DECL(LPSTR) GetCommandLineA(void);
DECL(unsigned int) GetSystemDirectoryA(LPSTR, unsigned int);
DECL(BOOL) CreateProcessA(LPCSTR, LPSTR, void *, void *, BOOL, DWORD, void *, LPCSTR, STARTUPINFOA *,
                          PROCESS_INFORMATION *);
DECL(BOOL) CloseHandle(HANDLE);
DECL(DWORD) WaitForSingleObject(HANDLE, DWORD);
DECL(BOOL) GetExitCodeProcess(HANDLE, DWORD *);
DECL(BOOL) TerminateProcess(HANDLE, unsigned int);
DECL(void) InitializeCriticalSection(CRITICAL_SECTION *);
DECL(void) EnterCriticalSection(CRITICAL_SECTION *);
DECL(void) LeaveCriticalSection(CRITICAL_SECTION *);

static SERVICE_STATUS_HANDLE status_handle;
static SERVICE_STATUS status;
static HANDLE child;
static int stopping;
/* guards status, child and stopping: the control handler runs on another thread than service_main */
static CRITICAL_SECTION lock;

static void report(DWORD state, DWORD win32, DWORD specific)
{
    EnterCriticalSection(&lock);
    status.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
    status.dwCurrentState = state;
    status.dwControlsAccepted = state == SERVICE_RUNNING ? SERVICE_ACCEPT_STOP : 0;
    status.dwWin32ExitCode = win32;
    status.dwServiceSpecificExitCode = specific;
    status.dwCheckPoint = 0;
    status.dwWaitHint = 0;
    SetServiceStatus(status_handle, &status);
    LeaveCriticalSection(&lock);
}

static void WINAPI handler(DWORD control)
{
    EnterCriticalSection(&lock);
    /* stop pending is only reported while running, so it can never follow stopped */
    if (control == SERVICE_CONTROL_STOP && status.dwCurrentState == SERVICE_RUNNING) {
        stopping = 1;
        if (child)
            TerminateProcess(child, 1);
        status.dwCurrentState = SERVICE_STOP_PENDING;
        status.dwControlsAccepted = 0;
        status.dwWaitHint = 10000;
    }
    SetServiceStatus(status_handle, &status);
    LeaveCriticalSection(&lock);
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
    EnterCriticalSection(&lock);
    child = pi.hProcess;
    if (stopping)
        TerminateProcess(child, 1);
    LeaveCriticalSection(&lock);
    WaitForSingleObject(pi.hProcess, INFINITE);
    if (!GetExitCodeProcess(pi.hProcess, &code))
        code = 1;
    /* the handle is closed under the lock, so the handler never uses a closed one */
    EnterCriticalSection(&lock);
    child = NULL;
    CloseHandle(pi.hProcess);
    LeaveCriticalSection(&lock);
    if (code)
        report(SERVICE_STOPPED, ERROR_SERVICE_SPECIFIC_ERROR, code);
    else
        report(SERVICE_STOPPED, 0, 0);
}

void svc_entry(void)
{
    SERVICE_TABLE_ENTRYA table[2];
    InitializeCriticalSection(&lock);
    table[0].lpServiceName = "";
    table[0].lpServiceProc = service_main;
    table[1].lpServiceName = NULL;
    table[1].lpServiceProc = NULL;
    StartServiceCtrlDispatcherA(table);
    ExitProcess(0);
}
