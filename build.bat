@echo off
REM Build the single Slowfield Windows screen saver from the Rust workspace.
setlocal EnableExtensions
pushd "%~dp0" >nul || exit /b 1

where cargo >nul 2>&1
if errorlevel 1 (
    echo ERROR: Rust/Cargo was not found on PATH. Install Rust via rustup.
    goto :failed
)

echo Building Slowfield...
cargo build --release --package slowfield
if errorlevel 1 goto :failed

if not exist "target\release\slowfield.exe" (
    echo ERROR: Cargo completed without target\release\slowfield.exe.
    goto :failed
)

if not exist "bin" mkdir "bin"
set "TMP_OUT=bin\slowfield-%RANDOM%-%RANDOM%.scr"
copy /y "target\release\slowfield.exe" "%TMP_OUT%" >nul
if errorlevel 1 goto :failed
move /y "%TMP_OUT%" "bin\slowfield.scr" >nul
if errorlevel 1 goto :failed

echo.
echo Build successful: bin\slowfield.scr
echo Run "bin\slowfield.scr /w" to preview in a window.
popd
endlocal
exit /b 0

:failed
if defined TMP_OUT if exist "%TMP_OUT%" del /q "%TMP_OUT%" >nul 2>&1
echo.
echo ERROR: Build failed. The existing bin\slowfield.scr was not replaced.
popd
endlocal
exit /b 1
