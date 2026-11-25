@echo off
REM Build the self-contained Rust Windows screen saver.
setlocal EnableExtensions
pushd "%~dp0" >nul || exit /b 1

where cargo >nul 2>&1
if errorlevel 1 (
    echo ERROR: Rust/Cargo was not found on PATH. Install Rust via rustup.
    goto :failed
)

echo Building the Rust renderer...
cargo build --release
if errorlevel 1 goto :failed

if not exist "target\release\ulam.exe" (
    echo ERROR: Cargo completed without target\release\ulam.exe.
    goto :failed
)

if not exist "bin" mkdir "bin"
set "TMP_OUT=bin\ulam-%RANDOM%-%RANDOM%.scr"
copy /y "target\release\ulam.exe" "%TMP_OUT%" >nul
if errorlevel 1 goto :failed
move /y "%TMP_OUT%" "bin\ulam.scr" >nul
if errorlevel 1 goto :failed

echo.
echo Build successful: bin\ulam.scr
echo Run "bin\ulam.scr /w" to preview in a window.
popd
endlocal
exit /b 0

:failed
if defined TMP_OUT if exist "%TMP_OUT%" del /q "%TMP_OUT%" >nul 2>&1
echo.
echo ERROR: Build failed. The existing bin\ulam.scr was not replaced.
popd
endlocal
exit /b 1
