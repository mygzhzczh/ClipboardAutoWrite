@echo off
chcp 65001 >nul

set "EXE_PATH=%~dp0clipboard_autowrite.exe"

echo.
echo  [1/2] 写入注册表开机启动...
reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v ClipboardAutoWrite /t REG_SZ /d "%EXE_PATH%" /f >nul
echo  注册表 OK

echo.
echo  [2/2] 创建计划任务（崩溃自动重启）...
schtasks /delete /tn "ClipboardAutoWrite_Wake" /f >nul 2>&1

set "XML_FILE=%TEMP%\clipboard_task.xml"
(
echo ^<?xml version="1.0" encoding="UTF-16"?^>
echo ^<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task"^>
echo   ^<RegistrationInfo^>
echo     ^<Description^>Clipboard Auto Write - restart on failure^</Description^>
echo   ^<^/RegistrationInfo^>
echo   ^<Triggers^>
echo     ^<LogonTrigger^>
echo       ^<Enabled^>true^</Enabled^>
echo     ^<^/LogonTrigger^>
echo   ^<^/Triggers^>
echo   ^<Principals^>
echo     ^<Principal id="Author"^>
echo       ^<LogonType^>InteractiveToken^</LogonType^>
echo       ^<RunLevel^>LeastPrivilege^</RunLevel^>
echo     ^<^/Principal^>
echo   ^<^/Principals^>
echo   ^<Settings^>
echo     ^<DisallowStartIfOnBatteries^>false^</DisallowStartIfOnBatteries^>
echo     ^<StopIfGoingOnBatteries^>false^</StopIfGoingOnBatteries^>
echo     ^<AllowHardTerminate^>true^</AllowHardTerminate^>
echo     ^<StartWhenAvailable^>true^</StartWhenAvailable^>
echo     ^<AllowStartOnDemand^>true^</AllowStartOnDemand^>
echo     ^<Enabled^>true^</Enabled^>
echo     ^<Hidden^>true^</Hidden^>
echo     ^<ExecutionTimeLimit^>PT0S^</ExecutionTimeLimit^>
echo     ^<RestartOnFailure^>
echo       ^<Interval^>PT1M^</Interval^>
echo       ^<Count^>3^</Count^>
echo     ^<^/RestartOnFailure^>
echo   ^<^/Settings^>
echo   ^<Actions Context="Author"^>
echo     ^<Exec^>
echo       ^<Command^>%EXE_PATH%^</Command^>
echo     ^<^/Exec^>
echo   ^<^/Actions^>
echo ^</Task^>
) > "%XML_FILE%"

schtasks /create /tn "ClipboardAutoWrite_Wake" /xml "%XML_FILE%" /f >nul 2>&1
del "%XML_FILE%" >nul 2>&1
echo  计划任务 OK
echo.
echo  全部完成！
pause