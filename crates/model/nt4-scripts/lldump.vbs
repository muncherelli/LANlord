' lldump.vbs: dumps the iis metabase of this machine as key=value lines.
' run it with: cscript //nologo lldump.vbs
' it needs only what vbscript 5 and the nt4 option pack wsh have: no regular expressions, no
' dictionaries, no with blocks.
'
' a property whose name holds "pass" (AnonymousUserPass, WAMUserPass and so on) is never printed:
' SafeGet and HasSecret are the only places that call Get. SafeGet looks at the name first and
' refuses these; HasSecret only says whether such a property has a value and returns that
' Boolean, never the value. the line for such a property reads <key>=<redacted> when it has a
' value and <key>=<unset> when it has none.
Option Explicit

Dim gServices, gSiteProps, gVdirProps, gFilterProps, gI, gService, gRoot

' the key of every line starts with the service, so the lines of one service sort together
gServices = Array("W3SVC", "MSFTPSVC", "SMTPSVC")

' properties of a service and of a site
gSiteProps = Array("KeyType", "ServerComment", "ServerBindings", "SecureBindings", _
  "ServerState", "ServerAutoStart", "HttpRedirect", "DefaultDoc", "FilterLoadOrder", _
  "AnonymousUserName", "AnonymousUserPass", "WAMUserName", "WAMUserPass", "LogType", _
  "LogOdbcUserName", "LogOdbcPassword", "LogPluginClsid", "AllowAnonymous", "AccessFlags", _
  "AuthFlags", "ScriptMaps", "AppIsolated", "AspEnableParentPaths", _
  "AspScriptErrorSentToBrowser", "EnableDirBrowsing")

' properties of a virtual directory or folder
gVdirProps = Array("KeyType", "Path", "AccessFlags", "AuthFlags", "ScriptMaps", "AppRoot", _
  "AppFriendlyName", "HttpRedirect", "DefaultDoc", "DirBrowseFlags", "AppIsolated", _
  "AspEnableParentPaths", "AspScriptErrorSentToBrowser", "EnableDirBrowsing", _
  "AnonymousUserName", "AnonymousUserPass", "WAMUserName", "WAMUserPass", "UNCUserName", _
  "UNCPassword")

' properties of an isapi filter
gFilterProps = Array("KeyType", "FilterPath", "FilterEnabled", "FilterState", _
  "FilterDescription", "FilterFlags")

' true when the name may hold a secret
Function IsSecret(sName)
  IsSecret = (InStr(1, sName, "pass", 1) > 0)
End Function

' one text line: a value never breaks the output
Function Clean(vValue)
  Dim s, n
  If IsNull(vValue) Then
    s = ""
  ElseIf IsObject(vValue) Then
    s = "<object>"
  ElseIf IsArray(vValue) Then
    s = ""
    For n = LBound(vValue) To UBound(vValue)
      If n > LBound(vValue) Then s = s & ","
      If IsArray(vValue(n)) Then
        s = s & "<array>"
      Else
        s = s & Clean(vValue(n))
      End If
    Next
  Else
    s = CStr(vValue)
  End If
  s = Replace(s, vbCr, " ")
  s = Replace(s, vbLf, " ")
  Clean = s
End Function

' true when a secret property has a value; the value itself goes no further than this function.
Function HasSecret(oObj, sName)
  Dim vValue, n
  HasSecret = False
  On Error Resume Next
  vValue = oObj.Get(sName)
  If Err.Number <> 0 Then
    Err.Clear
    vValue = Empty
  End If
  On Error GoTo 0
  If IsEmpty(vValue) Or IsNull(vValue) Then
    HasSecret = False
  ElseIf IsArray(vValue) Then
    HasSecret = (UBound(vValue) >= LBound(vValue))
  ElseIf IsObject(vValue) Then
    HasSecret = True
  Else
    HasSecret = (Len(CStr(vValue)) > 0)
  End If
  vValue = Empty
End Function

' the value of a property, or Empty when it has none. with HasSecret, the only Get in this file.
Function SafeGet(oObj, sName)
  SafeGet = Empty
  If IsSecret(sName) Then Exit Function
  On Error Resume Next
  SafeGet = oObj.Get(sName)
  If Err.Number <> 0 Then
    Err.Clear
    SafeGet = Empty
  End If
  On Error GoTo 0
End Function

' prints the lines for one property of oObj under the key sKey
Sub Show(oObj, sKey, sName)
  Dim vValue, n
  If IsSecret(sName) Then
    If HasSecret(oObj, sName) Then
      WScript.Echo sKey & "/" & sName & "=<redacted>"
    Else
      WScript.Echo sKey & "/" & sName & "=<unset>"
    End If
    Exit Sub
  End If
  vValue = SafeGet(oObj, sName)
  If IsEmpty(vValue) Then Exit Sub
  If IsArray(vValue) Then
    For n = LBound(vValue) To UBound(vValue)
      WScript.Echo sKey & "/" & sName & "[" & n & "]=" & Clean(vValue(n))
    Next
  Else
    WScript.Echo sKey & "/" & sName & "=" & Clean(vValue)
  End If
End Sub

Sub ShowAll(oObj, sKey, aNames)
  Dim n
  For n = LBound(aNames) To UBound(aNames)
    Show oObj, sKey, aNames(n)
  Next
End Sub

' walks the children of oNode: sites, directories and filters
Sub Walk(oNode, sKey, nDepth)
  Dim oChild, sClass, sChildKey
  If nDepth > 12 Then Exit Sub
  On Error Resume Next
  For Each oChild In oNode
    If Err.Number <> 0 Then
      Err.Clear
      Exit For
    End If
    sClass = oChild.Class
    sChildKey = sKey & "/" & oChild.Name
    WScript.Echo sChildKey & "/Class=" & sClass
    Select Case sClass
      Case "IIsWebServer", "IIsFtpServer", "IIsSmtpServer"
        ShowAll oChild, sChildKey, gSiteProps
        Walk oChild, sChildKey, nDepth + 1
      Case "IIsWebVirtualDir", "IIsWebDirectory", "IIsFtpVirtualDir", "IIsFtpDirectory"
        ShowAll oChild, sChildKey, gVdirProps
        Walk oChild, sChildKey, nDepth + 1
      Case "IIsFilters"
        ShowAll oChild, sChildKey, Array("KeyType", "FilterLoadOrder")
        Walk oChild, sChildKey, nDepth + 1
      Case "IIsFilter"
        ShowAll oChild, sChildKey, gFilterProps
      Case Else
        Walk oChild, sChildKey, nDepth + 1
    End Select
    Err.Clear
  Next
  On Error GoTo 0
End Sub

' present when the registry value can be read, absent when it cannot (the value or its key is
' not there). only for a value: a key's default value may be unset, which looks the same.
Function RegValueState(oShell, sPath)
  Dim vValue
  On Error Resume Next
  vValue = oShell.RegRead(sPath)
  If Err.Number = 0 Then
    RegValueState = "present"
  Else
    RegValueState = "absent"
  End If
  Err.Clear
  On Error GoTo 0
End Function

' present when a key's default value can be read, else unknown: on nt4 the error for a missing
' key and for a key with no default value is the same, so a failed read proves nothing.
Function RegKeyState(oShell, sPath)
  Dim vValue
  On Error Resume Next
  vValue = oShell.RegRead(sPath)
  If Err.Number = 0 Then
    RegKeyState = "present"
  Else
    RegKeyState = "unknown"
  End If
  Err.Clear
  On Error GoTo 0
End Function

' a registry string value, or "" when it cannot be read
Function RegText(oShell, sPath)
  Dim vValue
  RegText = ""
  On Error Resume Next
  vValue = oShell.RegRead(sPath)
  If Err.Number = 0 Then
    If Not IsArray(vValue) And Not IsNull(vValue) Then RegText = CStr(vValue)
  End If
  Err.Clear
  On Error GoTo 0
End Function

' false for an empty path or one with an unexpanded %variable%
Function PathOk(sPath)
  PathOk = (Len(sPath) > 0 And Left(sPath, 1) <> "%")
End Function

' base and rest joined, or "" when there is no base to put the rest under
Function Under(sBase, sRest)
  If PathOk(sBase) Then
    Under = sBase & sRest
  Else
    Under = ""
  End If
End Function

' the folder a default-site virtual directory points at, or "" when the metabase has none.
' this is what makes a custom web root (D:\Sites) work: the vdir path wins over the option pack's.
Function VdirPath(sNode)
  Dim oVdir, vPath
  VdirPath = ""
  Set oVdir = Nothing
  On Error Resume Next
  Set oVdir = GetObject("IIS://localhost/" & sNode)
  If Err.Number <> 0 Or oVdir Is Nothing Then
    Err.Clear
    On Error GoTo 0
    Exit Function
  End If
  On Error GoTo 0
  vPath = SafeGet(oVdir, "Path")
  If Not IsEmpty(vPath) And Not IsNull(vPath) And Not IsArray(vPath) Then
    If PathOk(CStr(vPath)) Then VdirPath = CStr(vPath)
  End If
End Function

Function FolderState(oFso, sPath)
  FolderState = "unknown"
  If Not PathOk(sPath) Then Exit Function
  On Error Resume Next
  If oFso.FolderExists(sPath) Then
    FolderState = "present"
  Else
    FolderState = "absent"
  End If
  If Err.Number <> 0 Then
    Err.Clear
    FolderState = "unknown"
  End If
  On Error GoTo 0
End Function

Function FileState(oFso, sPath)
  FileState = "unknown"
  If Not PathOk(sPath) Then Exit Function
  On Error Resume Next
  If oFso.FileExists(sPath) Then
    FileState = "present"
  Else
    FileState = "absent"
  End If
  If Err.Number <> 0 Then
    Err.Clear
    FileState = "unknown"
  End If
  On Error GoTo 0
End Function

' what the hardening checks need from the machine itself: files, folders and registry keys.
' the lines start with Host/, which is not a service, and say present, absent or unknown.
' nothing here reads a secret. nt4 has no %CommonProgramFiles%, so the common files folder
' comes from the registry, and the inetpub folder from the web root the option pack recorded.
Sub HostFacts()
  Dim oShell, oFso, sCommon, sInet, sSamples, sScripts, sReg, vStart, aNames, aAdc, n
  Set oShell = Nothing
  Set oFso = Nothing
  On Error Resume Next
  Set oShell = CreateObject("WScript.Shell")
  Set oFso = CreateObject("Scripting.FileSystemObject")
  If Err.Number <> 0 Or oShell Is Nothing Or oFso Is Nothing Then
    Err.Clear
    On Error GoTo 0
    Exit Sub
  End If
  On Error GoTo 0
  sCommon = RegText(oShell, "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\CommonFilesDir")
  sInet = RegText(oShell, "HKLM\SOFTWARE\Microsoft\InetStp\PathWWWRoot")
  If PathOk(sInet) Then
    On Error Resume Next
    sInet = oFso.GetParentFolderName(sInet)
    If Err.Number <> 0 Then
      Err.Clear
      sInet = ""
    End If
    On Error GoTo 0
  End If
  If Not PathOk(sCommon) Then sCommon = ""
  If Not PathOk(sInet) Then sInet = ""
  ' where the metabase puts the samples and scripts wins; the option pack's parent is the fallback
  sSamples = VdirPath("W3SVC/1/ROOT/IISSamples")
  If Not PathOk(sSamples) Then sSamples = Under(sInet, "\iissamples")
  sScripts = VdirPath("W3SVC/1/ROOT/Scripts")
  If Not PathOk(sScripts) Then sScripts = Under(sInet, "\scripts")
  WScript.Echo "Host/Folder/IISSamples=" & FolderState(oFso, sSamples)
  WScript.Echo "Host/Folder/ScriptsTools=" & FolderState(oFso, Under(sScripts, "\tools"))
  WScript.Echo "Host/File/codebrws.asp=" & _
    FileState(oFso, Under(sSamples, "\sdk\asp\docs\codebrws.asp"))
  WScript.Echo "Host/File/showcode.asp=" & _
    FileState(oFso, Under(sCommon, "\System\msadc\Samples\SELECTOR\showcode.asp"))
  WScript.Echo "Host/Folder/FPSE=" & _
    FolderState(oFso, Under(sCommon, "\Microsoft Shared\web server extensions"))
  WScript.Echo "Host/Reg/Hotfix/Q811114=" & _
    RegValueState(oShell, "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Hotfix\Q811114\Installed")
  sReg = "HKLM\SYSTEM\CurrentControlSet\Services\W3SVC\Parameters\ADCLaunch\"
  aAdc = Array("RDSServer.DataFactory", "AdvancedDataFactory", "VbBusObj.VbBusObjCls")
  For n = LBound(aAdc) To UBound(aAdc)
    WScript.Echo "Host/Reg/ADCLaunch/" & aAdc(n) & "=" & RegKeyState(oShell, sReg & aAdc(n) & "\")
  Next
  ' the start type of services a web server does not need: 2 automatic, 3 manual, 4 disabled
  aNames = Array("Alerter", "ClipSrv", "Messenger", "NetDDE", "RpcLocator", "SimpTcp", "TlntSvr")
  For n = LBound(aNames) To UBound(aNames)
    vStart = Empty
    On Error Resume Next
    vStart = oShell.RegRead("HKLM\SYSTEM\CurrentControlSet\Services\" & aNames(n) & "\Start")
    If Err.Number <> 0 Then
      Err.Clear
      vStart = Empty
    End If
    On Error GoTo 0
    If IsEmpty(vStart) Then
      WScript.Echo "Host/Svc/" & aNames(n) & "/Start=absent"
    Else
      WScript.Echo "Host/Svc/" & aNames(n) & "/Start=" & CStr(vStart)
    End If
  Next
End Sub

For gI = LBound(gServices) To UBound(gServices)
  gService = gServices(gI)
  Set gRoot = Nothing
  On Error Resume Next
  Set gRoot = GetObject("IIS://localhost/" & gService)
  If Err.Number <> 0 Or gRoot Is Nothing Then
    Err.Clear
    WScript.Echo gService & "=absent"
  Else
    WScript.Echo gService & "/Class=" & gRoot.Class
    ShowAll gRoot, gService, gSiteProps
    Walk gRoot, gService, 0
  End If
  On Error GoTo 0
Next

HostFacts
