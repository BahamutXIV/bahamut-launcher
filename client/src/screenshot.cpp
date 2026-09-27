#include "screenshot.h"

#include <wincodec.h>
#include <wrl/client.h>

#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cwchar>
#include <fstream>
#include <limits>
#include <string>
#include <string_view>
#include <system_error>
#include <vector>

namespace
{

using Microsoft::WRL::ComPtr;

std::filesystem::path gDirectory;
ScreenshotFormat      gFormat       = ScreenshotFormat::Png;
volatile LONG         gConfigured   = 0;
volatile LONG         gHideOverlays = 1;
volatile LONG         gPending      = 0;
volatile LONG         gSequence     = 0;

void RecordTestResult(std::string_view status, const std::filesystem::path& path)
{
    wchar_t     resultPath[32768]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_TEST_SCREENSHOT_RESULT_FILE", resultPath, ARRAYSIZE(resultPath));
    if (count == 0 || count >= ARRAYSIZE(resultPath))
    {
        return;
    }
    std::ofstream file(resultPath, std::ios::trunc);
    if (file)
    {
        file << "status=" << status << '\n';
        file << "path=" << path.string() << '\n';
        file << "hide_overlays="
             << (InterlockedCompareExchange(&gHideOverlays, 0, 0) != 0 ? 1 : 0)
             << '\n';
    }
}

void LogFailure(HRESULT result)
{
    char message[96]{};
    std::snprintf(message, sizeof(message), "Bahamut runtime: screenshot capture failed hr=0x%08lX\n", static_cast<unsigned long>(result));
    OutputDebugStringA(message);
}

std::filesystem::path NextPath()
{
    SYSTEMTIME time{};
    GetLocalTime(&time);
    const LONG sequence = InterlockedIncrement(&gSequence);
    wchar_t    name[96]{};
    std::swprintf(name, ARRAYSIZE(name), L"Bahamut_%04u%02u%02u_%02u%02u%02u_%03u_%ld.%ls", time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond, time.wMilliseconds, sequence, gFormat == ScreenshotFormat::Png ? L"png" : L"bmp");
    return gDirectory / name;
}

HRESULT ReadBackBuffer(IDirect3DDevice9* device, UINT& width, UINT& height, std::vector<std::uint8_t>& pixels)
{
    ComPtr<IDirect3DSurface9> backBuffer;
    HRESULT                   result = device->GetBackBuffer(
        0, 0, D3DBACKBUFFER_TYPE_MONO, backBuffer.GetAddressOf());
    if (FAILED(result))
    {
        return result;
    }

    D3DSURFACE_DESC description{};
    result = backBuffer->GetDesc(&description);
    if (FAILED(result))
    {
        return result;
    }
    if (description.Format != D3DFMT_A8R8G8B8 && description.Format != D3DFMT_X8R8G8B8)
    {
        return D3DERR_INVALIDCALL;
    }

    ComPtr<IDirect3DSurface9> resolved;
    IDirect3DSurface9*        source = backBuffer.Get();
    if (description.MultiSampleType != D3DMULTISAMPLE_NONE)
    {
        result = device->CreateRenderTarget(description.Width, description.Height, description.Format, D3DMULTISAMPLE_NONE, 0, FALSE, resolved.GetAddressOf(), nullptr);
        if (FAILED(result))
        {
            return result;
        }
        result = device->StretchRect(backBuffer.Get(), nullptr, resolved.Get(), nullptr, D3DTEXF_NONE);
        if (FAILED(result))
        {
            return result;
        }
        source = resolved.Get();
    }

    ComPtr<IDirect3DSurface9> readback;
    result = device->CreateOffscreenPlainSurface(description.Width,
                                                 description.Height,
                                                 description.Format,
                                                 D3DPOOL_SYSTEMMEM,
                                                 readback.GetAddressOf(),
                                                 nullptr);
    if (FAILED(result))
    {
        return result;
    }
    result = device->GetRenderTargetData(source, readback.Get());
    if (FAILED(result))
    {
        return result;
    }

    const std::uint64_t byteCount = static_cast<std::uint64_t>(description.Width) * static_cast<std::uint64_t>(description.Height) * 4;
    if (byteCount > std::numeric_limits<UINT>::max())
    {
        return E_OUTOFMEMORY;
    }
    pixels.resize(static_cast<std::size_t>(byteCount));
    D3DLOCKED_RECT locked{};
    result = readback->LockRect(&locked, nullptr, D3DLOCK_READONLY);
    if (FAILED(result))
    {
        return result;
    }
    for (UINT row = 0; row < description.Height; ++row)
    {
        const auto* sourceRow   = static_cast<const std::uint8_t*>(locked.pBits) + static_cast<std::size_t>(row) * locked.Pitch;
        auto*       destination = pixels.data() + static_cast<std::size_t>(row) * description.Width * 4;
        std::copy_n(sourceRow, static_cast<std::size_t>(description.Width) * 4, destination);
        for (UINT column = 0; column < description.Width; ++column)
        {
            destination[column * 4 + 3] = 0xff;
        }
    }
    readback->UnlockRect();
    width  = description.Width;
    height = description.Height;
    return S_OK;
}

HRESULT EncodeImage(const std::filesystem::path& path, UINT width, UINT height, const std::vector<std::uint8_t>& pixels)
{
    const HRESULT comResult    = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    const bool    uninitialize = SUCCEEDED(comResult);
    if (FAILED(comResult) && comResult != RPC_E_CHANGED_MODE)
    {
        return comResult;
    }

    struct ComCleanup
    {
        bool active;

        ~ComCleanup()
        {
            if (active)
                CoUninitialize();
        }
    } cleanup{ uninitialize };

    HRESULT                       result = S_OK;
    ComPtr<IWICImagingFactory>    factory;
    ComPtr<IWICStream>            stream;
    ComPtr<IWICBitmapEncoder>     encoder;
    ComPtr<IWICBitmapFrameEncode> frame;
    ComPtr<IPropertyBag2>         options;
    ComPtr<IWICBitmap>            bitmap;

    result = CoCreateInstance(CLSID_WICImagingFactory, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(factory.GetAddressOf()));
    if (SUCCEEDED(result))
        result = factory->CreateStream(stream.GetAddressOf());
    if (SUCCEEDED(result))
    {
        result = stream->InitializeFromFilename(path.c_str(), GENERIC_WRITE);
    }
    const GUID& container = gFormat == ScreenshotFormat::Png
                                ? GUID_ContainerFormatPng
                                : GUID_ContainerFormatBmp;
    if (SUCCEEDED(result))
    {
        result = factory->CreateEncoder(container, nullptr, encoder.GetAddressOf());
    }
    if (SUCCEEDED(result))
        result = encoder->Initialize(stream.Get(), WICBitmapEncoderNoCache);
    if (SUCCEEDED(result))
    {
        result = encoder->CreateNewFrame(frame.GetAddressOf(), options.GetAddressOf());
    }
    if (SUCCEEDED(result))
        result = frame->Initialize(options.Get());
    if (SUCCEEDED(result))
        result = frame->SetSize(width, height);
    WICPixelFormatGUID pixelFormat = GUID_WICPixelFormat32bppBGRA;
    if (SUCCEEDED(result))
        result = frame->SetPixelFormat(&pixelFormat);
    if (SUCCEEDED(result))
    {
        result = factory->CreateBitmapFromMemory(width, height, GUID_WICPixelFormat32bppBGRA, width * 4, static_cast<UINT>(pixels.size()), const_cast<BYTE*>(pixels.data()), bitmap.GetAddressOf());
    }
    if (SUCCEEDED(result))
        result = frame->WriteSource(bitmap.Get(), nullptr);
    if (SUCCEEDED(result))
        result = frame->Commit();
    if (SUCCEEDED(result))
        result = encoder->Commit();
    return result;
}

bool Capture(IDirect3DDevice9* device, const std::filesystem::path& path)
{
    UINT                      width  = 0;
    UINT                      height = 0;
    std::vector<std::uint8_t> pixels;
    HRESULT                   result = ReadBackBuffer(device, width, height, pixels);
    if (FAILED(result))
    {
        LogFailure(result);
        return false;
    }

    std::error_code directoryError;
    std::filesystem::create_directories(path.parent_path(), directoryError);
    if (directoryError)
    {
        return false;
    }
    std::filesystem::path staging = path;
    staging += L".tmp";
    result = EncodeImage(staging, width, height, pixels);
    if (SUCCEEDED(result) && !MoveFileExW(staging.c_str(), path.c_str(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH))
    {
        result = HRESULT_FROM_WIN32(GetLastError());
    }
    if (FAILED(result))
    {
        DeleteFileW(staging.c_str());
        LogFailure(result);
        return false;
    }
    return true;
}

} // namespace

void ConfigureScreenshotCapture(const std::filesystem::path& directory,
                                ScreenshotFormat             format,
                                bool                         hideOverlays)
{
    gDirectory = directory;
    gFormat    = format;
    InterlockedExchange(&gHideOverlays, hideOverlays ? 1 : 0);
    InterlockedExchange(&gConfigured, directory.empty() ? 0 : 1);
    InterlockedExchange(&gPending, 0);
}

bool RequestScreenshotCapture()
{
    if (!IsScreenshotCaptureConfigured())
    {
        return false;
    }
    InterlockedExchange(&gPending, 1);
    return true;
}

void CancelScreenshotCapture()
{
    InterlockedExchange(&gPending, 0);
}

bool IsScreenshotCaptureConfigured()
{
    return InterlockedCompareExchange(&gConfigured, 0, 0) != 0;
}

bool PendingScreenshotHidesOverlays()
{
    return InterlockedCompareExchange(&gPending, 0, 0) != 0 && InterlockedCompareExchange(&gHideOverlays, 0, 0) != 0;
}

bool CapturePendingScreenshot(IDirect3DDevice9* device)
{
    if (InterlockedExchange(&gPending, 0) == 0)
    {
        return true;
    }
    const std::filesystem::path path     = NextPath();
    const bool                  captured = device != nullptr && Capture(device, path);
    RecordTestResult(captured ? "captured" : "failed", path);
    return captured;
}
