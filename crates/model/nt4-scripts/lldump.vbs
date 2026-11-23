' lldump.vbs: dumps the iis metabase of this machine as key=value lines.
' run it with: cscript //nologo lldump.vbs
' it needs only what vbscript 5 and the nt4 option pack wsh have: no regular expressions, no
' dictionaries, no with blocks.
'
' a property whose name holds "pass" (AnonymousUserPass, WAMUserPass and so on) is never read:
' SafeGet is the only place that calls Get, and it looks at the name first. the line for such a
' property reads <key>=<redacted>.
Option Explicit

Dim gServices, gSiteProps, gVdirProps, gFilterProps, gI, gService, gRoot

' the key of every line starts with the service, so the lines of one service sort together
gServices = Array("W3SVC", "MSFTPSVC", "SMTPSVC")

' properties of a service and of a site
gSiteProps = Array("KeyType", "ServerComment", "ServerBindings", "SecureBindings", _
  "ServerState", "ServerAutoStart", "HttpRedirect", "DefaultDoc", "FilterLoadOrder", _
  "AnonymousUserName", "AnonymousUserPass", "WAMUserName", "WAMUserPass", "LogType", _
  "LogOdbcUserName", "LogOdbcPassword")

' properties of a virtual directory or folder
gVdirProps = Array("KeyType", "Path", "AccessFlags", "AuthFlags", "ScriptMaps", "AppRoot", _
  "AppFriendlyName", "HttpRedirect", "DefaultDoc", "DirBrowseFlags", _
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

' the value of a property, or Empty when it has none. the only Get in this file.
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
    WScript.Echo sKey & "/" & sName & "=<redacted>"
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
