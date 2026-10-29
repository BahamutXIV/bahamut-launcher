#include "overlay.h"
#include "addon_host.h"
#include "autologin.h"
#include "input.h"
#include "native_plugin_host.h"
#include "player_state.h"
#include "render_boundary.h"
#include "targetlines_probe.h"
#include "xhbar_icon.h"
#include "xhbar_runtime.h"

#include <imgui.h>
#include <imgui_impl_dx9.h>
#include <imgui_impl_win32.h>
#include <wincodec.h>
#include <wrl/client.h>

#include <algorithm>
#include <array>
#include <cctype>
#include <cfloat>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <map>
#include <string>
#include <string_view>
#include <unordered_map>
#include <vector>

namespace
{

constexpr std::size_t kMaximumRenderTargets   = 4;
volatile LONG         gBackBufferTestRecorded = 0;
ImFont*               gZoneRegionFont         = nullptr;
ImFont*               gZoneAreaFont           = nullptr;
ImFont*               gGearsetsFont           = nullptr;
ImFont*               gGearsetsTitleFont      = nullptr;

bool HasTestFault(const wchar_t* name)
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(name, value, ARRAYSIZE(value));
    return count != 0 && count < ARRAYSIZE(value) && value[0] != L'0';
}

void RecordTestResult(std::string_view text)
{
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_RESULT_FILE",
                                                path,
                                                ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << text;
    }
}

class CompleteDeviceState
{
public:
    explicit CompleteDeviceState(IDirect3DDevice9* device)
    : device_(device)
    {
        captured_ = Capture();
    }

    ~CompleteDeviceState()
    {
        if (captured_ && !restored_)
        {
            if (!Restore())
            {
                DisableOverlay();
                RecordTestResult(IsOverlayVisible()
                                     ? "overlay_restore=failed\nvisible=1\n"
                                     : "overlay_restore=failed\nvisible=0\n");
            }
        }
        ReleaseReferences();
    }

    CompleteDeviceState(const CompleteDeviceState&)            = delete;
    CompleteDeviceState& operator=(const CompleteDeviceState&) = delete;

    bool IsCaptured() const
    {
        return captured_;
    }

    bool BindBackBuffer()
    {
        IDirect3DSurface9* backBuffer = nullptr;
        if (FAILED(device_->GetBackBuffer(
                0, 0, D3DBACKBUFFER_TYPE_MONO, &backBuffer)) ||
            backBuffer == nullptr)
        {
            return false;
        }
        bool ok = SUCCEEDED(device_->SetDepthStencilSurface(nullptr)) && SUCCEEDED(device_->SetRenderTarget(0, backBuffer));
        backBuffer->Release();
        for (std::size_t index = 1; index < targetCount_; ++index)
        {
            ok = SUCCEEDED(device_->SetRenderTarget(
                     static_cast<DWORD>(index), nullptr)) &&
                 ok;
        }
        return ok;
    }

    bool IsBackBufferBound() const
    {
        IDirect3DSurface9* backBuffer   = nullptr;
        IDirect3DSurface9* renderTarget = nullptr;
        const bool         available    = SUCCEEDED(device_->GetBackBuffer(
                                              0, 0, D3DBACKBUFFER_TYPE_MONO, &backBuffer)) &&
                                          backBuffer != nullptr && SUCCEEDED(device_->GetRenderTarget(0, &renderTarget)) && renderTarget != nullptr;
        const bool         matches      = available && backBuffer == renderTarget;
        if (renderTarget != nullptr)
            renderTarget->Release();
        if (backBuffer != nullptr)
            backBuffer->Release();
        return matches;
    }

    bool Restore()
    {
        if (!captured_ || restored_)
        {
            return restored_;
        }
        bool ok = SUCCEEDED(stateBlock_->Apply());
        for (std::size_t index = 0; index < targetCount_; ++index)
        {
            if (renderTargetCaptured_[index])
            {
                ok = SUCCEEDED(device_->SetRenderTarget(
                         static_cast<DWORD>(index), renderTargets_[index])) &&
                     ok;
            }
        }
        if (depthStencilCaptured_)
        {
            ok = SUCCEEDED(device_->SetDepthStencilSurface(depthStencil_)) && ok;
        }
        // SetRenderTarget resets the viewport, so dependent state returns last.
        ok = SUCCEEDED(device_->SetViewport(&viewport_)) && ok;
        ok = SUCCEEDED(device_->SetScissorRect(&scissor_)) && ok;
        ok = SUCCEEDED(device_->SetTransform(D3DTS_WORLD, &world_)) && ok;
        ok = SUCCEEDED(device_->SetTransform(D3DTS_VIEW, &view_)) && ok;
        ok = SUCCEEDED(device_->SetTransform(D3DTS_PROJECTION, &projection_)) && ok;
        if (HasTestFault(L"BAHAMUT_TEST_OVERLAY_RESTORE_FAILURE"))
        {
            ok = false;
        }
        restored_ = ok;
        return ok;
    }

private:
    bool Capture()
    {
        if (FAILED(device_->CreateStateBlock(D3DSBT_ALL, &stateBlock_)) || stateBlock_ == nullptr || FAILED(stateBlock_->Capture()) || FAILED(device_->GetTransform(D3DTS_WORLD, &world_)) || FAILED(device_->GetTransform(D3DTS_VIEW, &view_)) || FAILED(device_->GetTransform(D3DTS_PROJECTION, &projection_)) || FAILED(device_->GetViewport(&viewport_)) || FAILED(device_->GetScissorRect(&scissor_)))
        {
            return false;
        }

        D3DCAPS9 capabilities{};
        if (FAILED(device_->GetDeviceCaps(&capabilities)))
        {
            return false;
        }
        targetCount_ = std::min<std::size_t>(capabilities.NumSimultaneousRTs,
                                             kMaximumRenderTargets);
        for (std::size_t index = 0; index < targetCount_; ++index)
        {
            const HRESULT result = device_->GetRenderTarget(
                static_cast<DWORD>(index), &renderTargets_[index]);
            if (result == D3DERR_NOTFOUND)
            {
                renderTargetCaptured_[index] = false;
            }
            else if (FAILED(result))
            {
                return false;
            }
            else
            {
                renderTargetCaptured_[index] = true;
            }
        }
        const HRESULT depthResult = device_->GetDepthStencilSurface(&depthStencil_);
        depthStencilCaptured_     = SUCCEEDED(depthResult) || depthResult == D3DERR_NOTFOUND;
        return depthStencilCaptured_;
    }

    void ReleaseReferences()
    {
        for (IDirect3DSurface9*& target : renderTargets_)
        {
            if (target != nullptr)
            {
                target->Release();
                target = nullptr;
            }
        }
        if (depthStencil_ != nullptr)
        {
            depthStencil_->Release();
            depthStencil_ = nullptr;
        }
        if (stateBlock_ != nullptr)
        {
            stateBlock_->Release();
            stateBlock_ = nullptr;
        }
    }

    IDirect3DDevice9*                                     device_     = nullptr;
    IDirect3DStateBlock9*                                 stateBlock_ = nullptr;
    std::array<IDirect3DSurface9*, kMaximumRenderTargets> renderTargets_{};
    std::array<bool, kMaximumRenderTargets>               renderTargetCaptured_{};
    std::size_t                                           targetCount_  = 0;
    IDirect3DSurface9*                                    depthStencil_ = nullptr;
    D3DMATRIX                                             world_{};
    D3DMATRIX                                             view_{};
    D3DMATRIX                                             projection_{};
    D3DVIEWPORT9                                          viewport_{};
    RECT                                                  scissor_{};
    bool                                                  depthStencilCaptured_ = false;
    bool                                                  captured_             = false;
    bool                                                  restored_             = false;
};

void RecordBackBufferTestResult(const CompleteDeviceState& state)
{
    if (InterlockedCompareExchange(&gBackBufferTestRecorded, 1, 0) != 0)
    {
        return;
    }
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_TEST_OVERLAY_TARGET_RESULT_FILE", path, ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << (state.IsBackBufferBound()
                     ? "overlay_target=backbuffer\n"
                     : "overlay_target=other\n");
    }
}

bool          gInitialized          = false;
bool          gBackendReady         = false;
LONG          gBackendRetryAttempt  = 0;
HWND          gWindow               = nullptr;
volatile LONG gOverlayDrawing       = 0;
volatile LONG gFillModeTestRecorded = 0;
// The first successfully hooked device owns all overlay state for the process.
// Other D3D9 device callbacks pass through without touching ImGui or telemetry.
IDirect3DDevice9* gOverlayDevice = nullptr;
LARGE_INTEGER     gLastAddonFrame{};
float             gNextAddonWindowY = 0.0f;
std::string       gDraggedAddonId;
bool              gSaveLayoutRequested = false;

std::filesystem::path& OverlayLayoutPath()
{
    static std::filesystem::path path;
    return path;
}

void SaveOverlayLayout()
{
    if (OverlayLayoutPath().empty() || ImGui::GetCurrentContext() == nullptr)
    {
        return;
    }
    std::size_t   size     = 0;
    const char*   settings = ImGui::SaveIniSettingsToMemory(&size);
    std::ofstream file(
        OverlayLayoutPath(), std::ios::binary | std::ios::trunc);
    if (file && settings != nullptr)
    {
        file.write(settings, static_cast<std::streamsize>(size));
    }
}

bool InitializeOverlay(IDirect3DDevice9* device)
{
    if (!IsOverlayDevice(device))
    {
        return false;
    }
    IMGUI_CHECKVERSION();
    gZoneRegionFont    = nullptr;
    gZoneAreaFont      = nullptr;
    gGearsetsFont      = nullptr;
    gGearsetsTitleFont = nullptr;
    ImGui::CreateContext();
    ImGuiIO& io    = ImGui::GetIO();
    io.IniFilename = nullptr;
    io.LogFilename = nullptr;
    io.ConfigFlags |= ImGuiConfigFlags_NavEnableKeyboard | ImGuiConfigFlags_NavEnableGamepad;
    ImGui::StyleColorsDark();
    io.Fonts->AddFontDefault();
    char       windowsDirectory[MAX_PATH]{};
    const UINT directoryLength = GetWindowsDirectoryA(windowsDirectory, MAX_PATH);
    if (directoryLength != 0 && directoryLength < MAX_PATH)
    {
        const std::string fontDirectory  = std::string(windowsDirectory) + "\\Fonts\\";
        const std::string regionFontPath = fontDirectory + "georgiab.ttf";
        const std::string areaFontPath   = fontDirectory + "georgiaz.ttf";
        if (std::filesystem::exists(regionFontPath))
        {
            gZoneRegionFont = io.Fonts->AddFontFromFileTTF(regionFontPath.c_str(), 13.0f);
        }
        if (std::filesystem::exists(areaFontPath))
        {
            gZoneAreaFont = io.Fonts->AddFontFromFileTTF(areaFontPath.c_str(), 30.0f);
        }
        // Retail 1.x menus use a serif face, with italic window titles.
        const std::string bodyFontPath  = fontDirectory + "georgia.ttf";
        const std::string titleFontPath = fontDirectory + "georgiai.ttf";
        if (std::filesystem::exists(bodyFontPath))
        {
            gGearsetsFont = io.Fonts->AddFontFromFileTTF(bodyFontPath.c_str(), 15.0f);
        }
        if (std::filesystem::exists(titleFontPath))
        {
            gGearsetsTitleFont = io.Fonts->AddFontFromFileTTF(titleFontPath.c_str(), 17.0f);
        }
    }
    if (!OverlayLayoutPath().empty())
    {
        std::ifstream     file(OverlayLayoutPath(), std::ios::binary);
        const std::string settings{
            std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()
        };
        if (!settings.empty())
        {
            ImGui::LoadIniSettingsFromMemory(settings.data(), settings.size());
        }
    }
    if (gWindow == nullptr || !ImGui_ImplWin32_Init(gWindow) || !ImGui_ImplDX9_Init(device))
    {
        if (io.BackendPlatformUserData != nullptr)
            ImGui_ImplWin32_Shutdown();
        ImGui::DestroyContext();
        return false;
    }
    gInitialized         = true;
    gBackendReady        = true;
    gBackendRetryAttempt = 0;
    return true;
}

// FFXIV 1.x window palette sampled from retail windows: navy panels with a
// pale blue rim, white serif text and gold accents.
constexpr ImVec4 kGearsetsText       = ImVec4(0.91f, 0.91f, 0.95f, 1.00f);
constexpr ImVec4 kGearsetsTitle      = ImVec4(0.94f, 0.92f, 0.84f, 1.00f);
constexpr ImVec4 kGearsetsDim        = ImVec4(0.58f, 0.62f, 0.78f, 1.00f);
constexpr ImVec4 kGearsetsCurrent    = ImVec4(0.96f, 0.90f, 0.68f, 1.00f);
constexpr ImU32  kGearsetsGold       = IM_COL32(217, 184, 98, 255);
constexpr ImU32  kGearsetsTop        = IM_COL32(30, 36, 82, 242);
constexpr ImU32  kGearsetsBottom     = IM_COL32(13, 14, 42, 242);
constexpr ImU32  kGearsetsFade       = IM_COL32(13, 14, 42, 0);
constexpr ImU32  kGearsetsRim        = IM_COL32(141, 159, 207, 255);
constexpr ImU32  kGearsetsInner      = IM_COL32(46, 59, 120, 255);
constexpr ImU32  kGearsetsChosen     = IM_COL32(70, 92, 178, 230);
constexpr ImU32  kGearsetsChosenFade = IM_COL32(70, 92, 178, 40);
constexpr ImU32  kGearsetsHover      = IM_COL32(48, 60, 128, 160);
constexpr ImU32  kGearsetsHoverFade  = IM_COL32(48, 60, 128, 16);
constexpr float  kGearsetsRow        = 34.0f;
constexpr float  kGearsetsIcon       = 28.0f;
constexpr int    kGearsetsColors     = 18;
constexpr int    kGearsetsVars       = 10;

char          gGearsetsRename[65]{};
std::uint32_t gGearsetsPopupRow = 0;
std::string   gGearsetsPopupName;
std::string   gGearsetsPopupContext;
// This frame's menu input claims, published to the input boundary after drawing.
bool gMenuMouseFrame    = false;
bool gMenuKeyboardFrame = false;

struct GearsetsIcon
{
    IDirect3DTexture9* texture   = nullptr;
    bool               attempted = false;
};

std::map<std::filesystem::path, GearsetsIcon> gGearsetsIcons;

// Decodes a small PNG into a managed texture, which survives device resets.
IDirect3DTexture9* LoadIconTexture(IDirect3DDevice9* device, const std::filesystem::path& path)
{
    std::error_code error;
    if (device == nullptr || !std::filesystem::is_regular_file(path, error))
        return nullptr;
    const HRESULT      comResult    = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    const bool         uninitialize = SUCCEEDED(comResult);
    IDirect3DTexture9* texture      = nullptr;
    {
        Microsoft::WRL::ComPtr<IWICImagingFactory>    factory;
        Microsoft::WRL::ComPtr<IWICBitmapDecoder>     decoder;
        Microsoft::WRL::ComPtr<IWICBitmapFrameDecode> frame;
        Microsoft::WRL::ComPtr<IWICFormatConverter>   converter;
        UINT                                          width  = 0;
        UINT                                          height = 0;
        if ((SUCCEEDED(comResult) || comResult == RPC_E_CHANGED_MODE) &&
            SUCCEEDED(CoCreateInstance(CLSID_WICImagingFactory, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(factory.GetAddressOf()))) &&
            SUCCEEDED(factory->CreateDecoderFromFilename(path.c_str(), nullptr, GENERIC_READ, WICDecodeMetadataCacheOnDemand, decoder.GetAddressOf())) &&
            SUCCEEDED(decoder->GetFrame(0, frame.GetAddressOf())) && SUCCEEDED(frame->GetSize(&width, &height)) &&
            width != 0 && height != 0 && width <= 128 && height <= 128 &&
            SUCCEEDED(factory->CreateFormatConverter(converter.GetAddressOf())) &&
            SUCCEEDED(converter->Initialize(frame.Get(), GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, nullptr, 0.0, WICBitmapPaletteTypeCustom)))
        {
            std::vector<BYTE> pixels(static_cast<std::size_t>(width) * height * 4u);
            D3DLOCKED_RECT    locked{};
            if (SUCCEEDED(converter->CopyPixels(nullptr, width * 4u, static_cast<UINT>(pixels.size()), pixels.data())) &&
                SUCCEEDED(device->CreateTexture(width, height, 1, 0, D3DFMT_A8R8G8B8, D3DPOOL_MANAGED, &texture, nullptr)))
            {
                if (SUCCEEDED(texture->LockRect(0, &locked, nullptr, 0)))
                {
                    for (UINT row = 0; row < height; ++row)
                        std::memcpy(static_cast<BYTE*>(locked.pBits) + static_cast<std::size_t>(row) * locked.Pitch, pixels.data() + static_cast<std::size_t>(row) * width * 4u, width * 4u);
                    texture->UnlockRect(0);
                }
                else
                {
                    texture->Release();
                    texture = nullptr;
                }
            }
        }
    }
    if (uninitialize)
        CoUninitialize();
    return texture;
}

// Abbreviations are A-Z only (the host checks), so they are safe file names.
// Each path is tried once; a missing icon falls back to the abbreviation alone.
IDirect3DTexture9* GearsetsIconTexture(const std::filesystem::path& directory, const std::string& abbreviation)
{
    if (abbreviation.empty() || directory.empty())
        return nullptr;
    std::string file = abbreviation;
    std::transform(file.begin(), file.end(), file.begin(), [](unsigned char value)
                   {
                       return static_cast<char>(std::tolower(value));
                   });
    const auto path = directory / (file + ".png");
    auto&      icon = gGearsetsIcons[path];
    if (!icon.attempted)
    {
        icon.attempted = true;
        icon.texture   = LoadIconTexture(gOverlayDevice, path);
    }
    return icon.texture;
}

void PushGearsetsStyle()
{
    ImGui::PushStyleColor(ImGuiCol_Text, kGearsetsText);
    ImGui::PushStyleColor(ImGuiCol_TextDisabled, kGearsetsDim);
    ImGui::PushStyleColor(ImGuiCol_WindowBg, ImVec4(0.0f, 0.0f, 0.0f, 0.0f));
    ImGui::PushStyleColor(ImGuiCol_ChildBg, ImVec4(0.04f, 0.04f, 0.15f, 0.92f));
    ImGui::PushStyleColor(ImGuiCol_PopupBg, ImVec4(0.06f, 0.07f, 0.20f, 0.97f));
    ImGui::PushStyleColor(ImGuiCol_Border, ImVec4(0.18f, 0.23f, 0.47f, 1.00f));
    ImGui::PushStyleColor(ImGuiCol_FrameBg, ImVec4(0.04f, 0.04f, 0.15f, 1.00f));
    ImGui::PushStyleColor(ImGuiCol_FrameBgHovered, ImVec4(0.09f, 0.11f, 0.27f, 1.00f));
    ImGui::PushStyleColor(ImGuiCol_FrameBgActive, ImVec4(0.12f, 0.15f, 0.34f, 1.00f));
    ImGui::PushStyleColor(ImGuiCol_Button, ImVec4(0.14f, 0.16f, 0.33f, 1.00f));
    ImGui::PushStyleColor(ImGuiCol_ButtonHovered, ImVec4(0.22f, 0.26f, 0.50f, 1.00f));
    ImGui::PushStyleColor(ImGuiCol_ButtonActive, ImVec4(0.29f, 0.35f, 0.64f, 1.00f));
    ImGui::PushStyleColor(ImGuiCol_Header, ImVec4(0.27f, 0.36f, 0.70f, 0.90f));
    ImGui::PushStyleColor(ImGuiCol_HeaderHovered, ImVec4(0.19f, 0.24f, 0.50f, 0.70f));
    ImGui::PushStyleColor(ImGuiCol_HeaderActive, ImVec4(0.31f, 0.40f, 0.76f, 0.90f));
    ImGui::PushStyleColor(ImGuiCol_Separator, ImVec4(0.18f, 0.23f, 0.47f, 1.00f));
    ImGui::PushStyleColor(ImGuiCol_ScrollbarBg, ImVec4(0.0f, 0.0f, 0.0f, 0.0f));
    ImGui::PushStyleColor(ImGuiCol_ModalWindowDimBg, ImVec4(0.0f, 0.0f, 0.0f, 0.45f));
    ImGui::PushStyleVar(ImGuiStyleVar_WindowBorderSize, 0.0f);
    ImGui::PushStyleVar(ImGuiStyleVar_ChildBorderSize, 1.0f);
    ImGui::PushStyleVar(ImGuiStyleVar_PopupBorderSize, 1.0f);
    ImGui::PushStyleVar(ImGuiStyleVar_FrameBorderSize, 1.0f);
    ImGui::PushStyleVar(ImGuiStyleVar_WindowRounding, 7.0f);
    ImGui::PushStyleVar(ImGuiStyleVar_ChildRounding, 4.0f);
    ImGui::PushStyleVar(ImGuiStyleVar_PopupRounding, 4.0f);
    ImGui::PushStyleVar(ImGuiStyleVar_FrameRounding, 9.0f);
    ImGui::PushStyleVar(ImGuiStyleVar_WindowPadding, ImVec2(11.0f, 9.0f));
    ImGui::PushStyleVar(ImGuiStyleVar_ItemSpacing, ImVec2(6.0f, 6.0f));
    if (gGearsetsFont != nullptr)
        ImGui::PushFont(gGearsetsFont);
}

void PopGearsetsStyle()
{
    if (gGearsetsFont != nullptr)
        ImGui::PopFont();
    ImGui::PopStyleVar(kGearsetsVars);
    ImGui::PopStyleColor(kGearsetsColors);
}

// The four-point star that precedes retail window titles.
void GearsetsStar(ImDrawList* draw, ImVec2 center, float radius)
{
    const float  inner    = radius * 0.32f;
    const ImVec2 points[] = {
        { center.x, center.y - radius },
        { center.x + inner, center.y - inner },
        { center.x + radius, center.y },
        { center.x + inner, center.y + inner },
        { center.x, center.y + radius },
        { center.x - inner, center.y + inner },
        { center.x - radius, center.y },
        { center.x - inner, center.y - inner },
    };
    draw->AddConvexPolyFilled(points, 8, kGearsetsGold);
}

// Background, double rim and title rule; returns the title row height.
float DrawGearsetsFrame(ImDrawList* draw, ImVec2 position, ImVec2 size)
{
    const float  titleHeight = 26.0f;
    const ImVec2 end(position.x + size.x, position.y + size.y);
    draw->AddRectFilled(position, end, kGearsetsBottom, 7.0f);
    draw->AddRectFilledMultiColor(ImVec2(position.x + 3.0f, position.y + 7.0f), ImVec2(end.x - 3.0f, position.y + size.y * 0.55f), kGearsetsTop, kGearsetsTop, kGearsetsFade, kGearsetsFade);
    draw->AddRect(position, end, kGearsetsRim, 7.0f, 0, 1.5f);
    draw->AddRect(ImVec2(position.x + 3.0f, position.y + 3.0f), ImVec2(end.x - 3.0f, end.y - 3.0f), kGearsetsInner, 5.0f);
    const float lineY = position.y + titleHeight + 4.0f;
    draw->AddLine(ImVec2(position.x + 8.0f, lineY), ImVec2(end.x - 8.0f, lineY), kGearsetsInner, 1.0f);
    return titleHeight;
}

enum class GearsetsGlyph
{
    Plus,
    Up,
    Down,
};

bool GearsetsGlyphButton(const char* id, GearsetsGlyph glyph, bool enabled)
{
    const float size = ImGui::GetFrameHeight();
    ImGui::BeginDisabled(!enabled);
    const bool pressed = ImGui::Button(id, ImVec2(size, size));
    ImGui::EndDisabled();
    const ImVec2 min = ImGui::GetItemRectMin();
    const ImVec2 center(min.x + size * 0.5f, min.y + size * 0.5f);
    const ImU32  color = enabled ? IM_COL32(232, 232, 242, 255) : IM_COL32(110, 118, 160, 255);
    auto*        draw  = ImGui::GetWindowDrawList();
    const float  arm   = size * 0.24f;
    if (glyph == GearsetsGlyph::Plus)
    {
        draw->AddLine(ImVec2(center.x - arm, center.y), ImVec2(center.x + arm, center.y), color, 2.0f);
        draw->AddLine(ImVec2(center.x, center.y - arm), ImVec2(center.x, center.y + arm), color, 2.0f);
    }
    else
    {
        const float tip = glyph == GearsetsGlyph::Up ? -arm : arm;
        draw->AddTriangleFilled(ImVec2(center.x, center.y + tip), ImVec2(center.x + arm, center.y - tip * 0.6f), ImVec2(center.x - arm, center.y - tip * 0.6f), color);
    }
    return pressed && enabled;
}

AddonGearsetsAction DrawGearsets(void*, const AddonGearsetsWindow& window)
{
    AddonGearsetsAction action;
    bool                openRename = false;
    bool                openDelete = false;
    PushGearsetsStyle();
    const ImGuiViewport* viewport = ImGui::GetMainViewport();
    ImGui::SetNextWindowPos(ImVec2(viewport->WorkPos.x + viewport->WorkSize.x * 0.70f, viewport->WorkPos.y + viewport->WorkSize.y * 0.18f), ImGuiCond_FirstUseEver);
    ImGui::SetNextWindowSize(ImVec2(270.0f, 430.0f), ImGuiCond_FirstUseEver);
    ImGui::SetNextWindowSizeConstraints(ImVec2(230.0f, 240.0f), ImVec2(520.0f, FLT_MAX));
    if (ImGui::Begin("Gear Sets###bahamut-addon-gearsets", nullptr, ImGuiWindowFlags_NoTitleBar | ImGuiWindowFlags_NoCollapse | ImGuiWindowFlags_NoScrollbar | ImGuiWindowFlags_NoScrollWithMouse))
    {
        auto*        draw     = ImGui::GetWindowDrawList();
        const ImVec2 position = ImGui::GetWindowPos();
        const ImVec2 size     = ImGui::GetWindowSize();
        const float  title    = DrawGearsetsFrame(draw, position, size);

        GearsetsStar(draw, ImVec2(position.x + 16.0f, position.y + 17.0f), 5.5f);
        if (gGearsetsTitleFont != nullptr)
            ImGui::PushFont(gGearsetsTitleFont);
        ImGui::SetCursorScreenPos(ImVec2(position.x + 26.0f, position.y + 7.0f));
        ImGui::TextColored(kGearsetsTitle, "Gear Sets");
        if (gGearsetsTitleFont != nullptr)
            ImGui::PopFont();
        ImGui::SetCursorScreenPos(ImVec2(position.x + size.x - 28.0f, position.y + 8.0f));
        if (ImGui::InvisibleButton("##close", ImVec2(18.0f, 18.0f)))
            action = { "close", 0, {} };
        const ImVec2 closeMin = ImGui::GetItemRectMin();
        const ImU32  closeInk = ImGui::IsItemHovered() ? IM_COL32(255, 255, 255, 255) : IM_COL32(180, 190, 225, 255);
        draw->AddLine(ImVec2(closeMin.x + 5.0f, closeMin.y + 5.0f), ImVec2(closeMin.x + 13.0f, closeMin.y + 13.0f), closeInk, 1.6f);
        draw->AddLine(ImVec2(closeMin.x + 13.0f, closeMin.y + 5.0f), ImVec2(closeMin.x + 5.0f, closeMin.y + 13.0f), closeInk, 1.6f);

        ImGui::SetCursorScreenPos(ImVec2(position.x + 11.0f, position.y + title + 11.0f));
        const std::size_t   count  = window.rows.size();
        const std::uint32_t chosen = window.selected <= count ? window.selected : 0;
        if (GearsetsGlyphButton("##save", GearsetsGlyph::Plus, window.editable && count < window.maximum && !window.equipping))
            action = { "save", 0, {} };
        if (ImGui::IsItemHovered(ImGuiHoveredFlags_AllowWhenDisabled))
            ImGui::SetTooltip("Save current gear as a new set");
        ImGui::SameLine();
        if (GearsetsGlyphButton("##up", GearsetsGlyph::Up, window.editable && chosen > 1))
            action = { "move", chosen, "up" };
        ImGui::SameLine();
        if (GearsetsGlyphButton("##down", GearsetsGlyph::Down, window.editable && chosen != 0 && chosen < count))
            action = { "move", chosen, "down" };

        // The status line is always reserved so the list never moves or resizes.
        const float footer = ImGui::GetFrameHeightWithSpacing() + ImGui::GetTextLineHeightWithSpacing();
        if (ImGui::BeginChild("##list", ImVec2(0.0f, -footer), ImGuiChildFlags_Borders))
        {
            auto* list = ImGui::GetWindowDrawList();
            for (std::size_t index = 0; index < count; ++index)
            {
                const auto&         row    = window.rows[index];
                const std::uint32_t number = static_cast<std::uint32_t>(index + 1);
                ImGui::PushID(static_cast<int>(index));
                const ImVec2 start = ImGui::GetCursorScreenPos();
                const float  width = ImGui::GetContentRegionAvail().x;
                if (ImGui::InvisibleButton("##row", ImVec2(width, kGearsetsRow)))
                    action = { "select", number, {} };
                if (ImGui::IsItemHovered() && ImGui::IsMouseDoubleClicked(ImGuiMouseButton_Left) && !window.equipping)
                    action = { "equip", number, {} };
                const ImVec2 end(start.x + width, start.y + kGearsetsRow);
                if (number == chosen)
                    list->AddRectFilledMultiColor(start, end, kGearsetsChosen, kGearsetsChosenFade, kGearsetsChosenFade, kGearsetsChosen);
                else if (ImGui::IsItemHovered())
                    list->AddRectFilledMultiColor(start, end, kGearsetsHover, kGearsetsHoverFade, kGearsetsHoverFade, kGearsetsHover);
                const float middle = start.y + kGearsetsRow * 0.5f;
                if (row.current)
                    GearsetsStar(list, ImVec2(start.x + 7.0f, middle), 4.5f);
                char label[8]{};
                std::snprintf(label, sizeof(label), "%u", number);
                const ImVec2 numberSize = ImGui::CalcTextSize(label);
                list->AddText(ImVec2(start.x + 32.0f - numberSize.x, middle - numberSize.y * 0.5f), ImGui::GetColorU32(kGearsetsText), label);
                const ImVec2 iconMin(start.x + 38.0f, middle - kGearsetsIcon * 0.5f);
                if (auto* texture = GearsetsIconTexture(window.iconDirectory, row.abbreviation))
                    list->AddImage(static_cast<ImTextureID>(reinterpret_cast<std::uintptr_t>(texture)), iconMin, ImVec2(iconMin.x + kGearsetsIcon, iconMin.y + kGearsetsIcon));
                const float  textX     = iconMin.x + kGearsetsIcon + 8.0f;
                const char*  shortName = row.abbreviation.empty() ? "---" : row.abbreviation.c_str();
                const ImVec2 textSize  = ImGui::CalcTextSize(shortName);
                list->AddText(ImVec2(textX, middle - textSize.y * 0.5f), ImGui::GetColorU32(kGearsetsText), shortName);
                list->PushClipRect(ImVec2(textX + 44.0f, start.y), ImVec2(end.x - 4.0f, end.y), true);
                list->AddText(ImVec2(textX + 44.0f, middle - textSize.y * 0.5f), ImGui::GetColorU32(row.current ? kGearsetsCurrent : kGearsetsText), row.name.c_str());
                list->PopClipRect();
                if (ImGui::BeginPopupContextItem("##actions"))
                {
                    if (ImGui::MenuItem("Equip", nullptr, false, !window.equipping))
                        action = { "equip", number, {} };
                    if (ImGui::MenuItem("Update from Current", nullptr, false, window.editable && !window.equipping))
                        action = { "update", number, {} };
                    if (ImGui::MenuItem("Rename...", nullptr, false, window.editable))
                        openRename = true;
                    ImGui::Separator();
                    if (ImGui::MenuItem("Delete...", nullptr, false, window.editable))
                        openDelete = true;
                    if (openRename || openDelete)
                    {
                        gGearsetsPopupRow     = number;
                        gGearsetsPopupName    = row.name;
                        gGearsetsPopupContext = window.context;
                        std::snprintf(gGearsetsRename, sizeof(gGearsetsRename), "%s", row.name.c_str());
                    }
                    ImGui::EndPopup();
                }
                ImGui::PopID();
            }
        }
        ImGui::EndChild();

        std::string    statusLine  = window.status;
        const float    statusWidth = ImGui::GetContentRegionAvail().x;
        constexpr char kEllipsis[] = "...";
        if (ImGui::CalcTextSize(statusLine.c_str()).x > statusWidth)
        {
            while (!statusLine.empty() && ImGui::CalcTextSize((statusLine + kEllipsis).c_str()).x > statusWidth)
            {
                // Drop one whole UTF-8 sequence.
                while (!statusLine.empty() && (static_cast<unsigned char>(statusLine.back()) & 0xC0u) == 0x80u)
                    statusLine.pop_back();
                if (!statusLine.empty())
                    statusLine.pop_back();
            }
            statusLine += kEllipsis;
        }
        ImGui::TextUnformatted(statusLine.c_str());
        if (statusLine != window.status && ImGui::IsItemHovered())
            ImGui::SetTooltip("%s", window.status.c_str());
        char countText[16]{};
        std::snprintf(countText, sizeof(countText), "%u/%u", static_cast<unsigned>(count), window.maximum);
        const float countWidth  = ImGui::CalcTextSize(countText).x;
        const float buttonWidth = std::max(96.0f, ImGui::GetContentRegionAvail().x - countWidth - 16.0f);
        if (window.equipping)
        {
            if (ImGui::Button("Cancel", ImVec2(buttonWidth, 0.0f)))
                action = { "cancel", 0, {} };
        }
        else
        {
            ImGui::BeginDisabled(chosen == 0);
            if (ImGui::Button("Equip Set", ImVec2(buttonWidth, 0.0f)))
                action = { "equip", chosen, {} };
            ImGui::EndDisabled();
        }
        ImGui::SameLine(ImGui::GetWindowContentRegionMax().x - countWidth);
        ImGui::AlignTextToFramePadding();
        ImGui::TextColored(kGearsetsDim, "%s", countText);

        if (openRename)
            ImGui::OpenPopup("Rename Set");
        if (openDelete)
            ImGui::OpenPopup("Delete Set");
        if (ImGui::BeginPopupModal("Rename Set", nullptr, ImGuiWindowFlags_AlwaysAutoResize))
        {
            if (window.context != gGearsetsPopupContext)
                ImGui::CloseCurrentPopup();
            // The dialog claims the keyboard from the frame it opens, before the
            // text field becomes active.
            gMenuKeyboardFrame = true;
            if (ImGui::IsWindowAppearing())
                ImGui::SetKeyboardFocusHere();
            ImGui::SetNextItemWidth(240.0f);
            const bool entered = ImGui::InputText("##rename", gGearsetsRename, sizeof(gGearsetsRename), ImGuiInputTextFlags_EnterReturnsTrue);
            if (ImGui::IsKeyPressed(ImGuiKey_Escape, false))
                ImGui::CloseCurrentPopup();
            if (ImGui::Button("Rename", ImVec2(96.0f, 0.0f)) || entered)
            {
                if (window.context == gGearsetsPopupContext)
                    action = { "rename", gGearsetsPopupRow, gGearsetsRename };
                ImGui::CloseCurrentPopup();
            }
            ImGui::SameLine();
            if (ImGui::Button("Close", ImVec2(96.0f, 0.0f)))
                ImGui::CloseCurrentPopup();
            ImGui::EndPopup();
        }
        if (ImGui::BeginPopupModal("Delete Set", nullptr, ImGuiWindowFlags_AlwaysAutoResize))
        {
            if (window.context != gGearsetsPopupContext)
                ImGui::CloseCurrentPopup();
            ImGui::Text("Delete \"%s\"?", gGearsetsPopupName.c_str());
            if (ImGui::Button("Delete", ImVec2(96.0f, 0.0f)))
            {
                if (window.context == gGearsetsPopupContext)
                    action = { "delete", gGearsetsPopupRow, {} };
                ImGui::CloseCurrentPopup();
            }
            ImGui::SameLine();
            if (ImGui::Button("Close", ImVec2(96.0f, 0.0f)))
                ImGui::CloseCurrentPopup();
            ImGui::EndPopup();
        }
        // The menu takes clicks over itself, and anywhere while a menu or dialog is open.
        gMenuMouseFrame = gMenuMouseFrame || ImGui::IsWindowHovered(ImGuiHoveredFlags_ChildWindows | ImGuiHoveredFlags_AllowWhenBlockedByActiveItem) ||
                          ImGui::IsPopupOpen(nullptr, ImGuiPopupFlags_AnyPopupId | ImGuiPopupFlags_AnyPopupLevel);
    }
    ImGui::End();
    PopGearsetsStyle();
    return action;
}

void DrawAddonWindow(void* context, const char* addonId, const char* title, const char* text, bool locked)
{
    UNREFERENCED_PARAMETER(context);
    const ImGuiViewport*   viewport   = ImGui::GetMainViewport();
    const bool             headerless = title[0] == '\0';
    const std::string_view id(addonId);
    if (id == "combatparser")
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + 28.0f,
                                    viewport->WorkPos.y + viewport->WorkSize.y * 0.5f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(0.0f, 0.5f));
        ImGui::SetNextWindowSizeConstraints(ImVec2(380.0f, 0.0f), ImVec2(FLT_MAX, FLT_MAX));
    }
    else if (id == "targethp" || id == "customtimer")
    {
        const float horizontalPosition = id == "targethp" ? 0.25f : 0.75f;
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x * horizontalPosition,
                                    viewport->WorkPos.y + 16.0f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(0.5f, 0.0f));
    }
    else if (id == "distance")
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x - 28.0f,
                                    viewport->WorkPos.y + viewport->WorkSize.y * 0.5f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(1.0f, 0.5f));
    }
    else if (id == "enemycast")
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x * 0.75f,
                                    viewport->WorkPos.y + 80.0f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(0.5f, 0.0f));
    }
    else if (headerless)
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + 28.0f,
                                    viewport->WorkPos.y + viewport->WorkSize.y - 8.0f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(0.0f, 1.0f));
    }
    else
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x - 16.0f,
                                    gNextAddonWindowY),
                                ImGuiCond_FirstUseEver,
                                ImVec2(1.0f, 0.0f));
    }
    ImGui::SetNextWindowBgAlpha(0.82f);
    const std::string windowName = std::string(title) + "###bahamut-addon-" + addonId;
    ImGuiWindowFlags  flags      = ImGuiWindowFlags_AlwaysAutoResize | ImGuiWindowFlags_NoCollapse | ImGuiWindowFlags_NoMove;
    if (headerless)
    {
        flags |= ImGuiWindowFlags_NoTitleBar;
    }
    if (locked || !ImGui::GetIO().KeyShift)
    {
        flags |= ImGuiWindowFlags_NoInputs;
    }
    if (ImGui::Begin(windowName.c_str(), nullptr, flags))
    {
        if (text[0] != '\0')
        {
            ImGui::TextUnformatted(text);
        }
    }
    const ImGuiIO& io = ImGui::GetIO();
    if (!locked && io.KeyShift && ImGui::IsWindowHovered(ImGuiHoveredFlags_AllowWhenBlockedByActiveItem) && ImGui::IsMouseClicked(ImGuiMouseButton_Left))
    {
        gDraggedAddonId = addonId;
    }
    if (locked && gDraggedAddonId == addonId)
    {
        gDraggedAddonId.clear();
    }
    else if (gDraggedAddonId == addonId)
    {
        if (!io.KeyShift || !ImGui::IsMouseDown(ImGuiMouseButton_Left))
        {
            gDraggedAddonId.clear();
            gSaveLayoutRequested = true;
        }
        else
        {
            const ImVec2 position = ImGui::GetWindowPos();
            ImGui::SetWindowPos(ImVec2(
                position.x + io.MouseDelta.x, position.y + io.MouseDelta.y));
        }
    }
    if (!headerless)
    {
        gNextAddonWindowY += ImGui::GetWindowSize().y + ImGui::GetStyle().ItemSpacing.y;
    }
    ImGui::End();
}

ImU32 ParseMeterColor(std::string_view value)
{
    const auto hexDigit = [](unsigned char character)
    {
        if (character >= '0' && character <= '9')
        {
            return static_cast<unsigned int>(character - '0');
        }
        if (character >= 'a' && character <= 'f')
        {
            return static_cast<unsigned int>(character - 'a' + 10);
        }
        if (character >= 'A' && character <= 'F')
        {
            return static_cast<unsigned int>(character - 'A' + 10);
        }
        return 0u;
    };
    if (value.size() != 7 || value[0] != '#')
    {
        return IM_COL32(160, 168, 180, 255);
    }
    const auto channel = [&](std::size_t index)
    {
        return hexDigit(static_cast<unsigned char>(value[index])) * 16u +
               hexDigit(static_cast<unsigned char>(value[index + 1u]));
    };
    return IM_COL32(channel(1), channel(3), channel(5), 255);
}

void DrawAddonCombatMeter(void* context, const char* addonId, const char* mode, const AddonCombatMeterRow* rows, std::size_t rowCount, bool locked, bool incomplete)
{
    UNREFERENCED_PARAMETER(context);
    const ImGuiViewport* viewport = ImGui::GetMainViewport();
    ImGui::SetNextWindowPos(ImVec2(viewport->WorkPos.x + 28.0f,
                                   viewport->WorkPos.y + viewport->WorkSize.y * 0.5f),
                            ImGuiCond_FirstUseEver,
                            ImVec2(0.0f, 0.5f));
    ImGui::SetNextWindowSize(ImVec2(392.0f, 0.0f), ImGuiCond_FirstUseEver);
    ImGui::SetNextWindowSizeConstraints(ImVec2(392.0f, 0.0f), ImVec2(392.0f, FLT_MAX));
    ImGui::SetNextWindowBgAlpha(0.94f);
    const std::string windowName = "###bahamut-addon-" + std::string(addonId);
    ImGuiWindowFlags  flags      = ImGuiWindowFlags_AlwaysAutoResize |
                                   ImGuiWindowFlags_NoCollapse |
                                   ImGuiWindowFlags_NoMove |
                                   ImGuiWindowFlags_NoTitleBar;
    if (locked || !ImGui::GetIO().KeyShift)
    {
        flags |= ImGuiWindowFlags_NoInputs;
    }
    ImGui::PushStyleColor(ImGuiCol_WindowBg, ImVec4(0.035f, 0.045f, 0.065f, 0.94f));
    if (ImGui::Begin(windowName.c_str(), nullptr, flags))
    {
        const bool  idle    = rowCount == 0 || rows == nullptr;
        const char* heading = std::string_view(mode) == "hps" ? "HPS" : "DPS";
        ImGui::TextColored(ImVec4(0.60f, 0.80f, 0.96f, 1.0f), "%s", heading);
        if (!idle)
        {
            ImGui::Separator();
            double maximumRate = 0.0;
            for (std::size_t index = 0; index < rowCount; ++index)
            {
                if (std::isfinite(rows[index].rate) && rows[index].rate > maximumRate)
                {
                    maximumRate = rows[index].rate;
                }
            }
            for (std::size_t index = 0; index < rowCount; ++index)
            {
                const AddonCombatMeterRow& row = rows[index];
                ImGui::PushID(static_cast<int>(index));
                char metric[128]{};
                char rate[32]{};
                if (row.rate >= 10000000.0)
                {
                    std::snprintf(rate, sizeof(rate), "%.1e", row.rate);
                }
                else
                {
                    std::snprintf(rate, sizeof(rate), "%.1f", row.rate);
                }
                const bool healingMode = std::string_view(mode) == "hps";
                if (healingMode)
                {
                    std::snprintf(metric, sizeof(metric), "%s HPS | %llu HEAL", rate, static_cast<unsigned long long>(row.amount));
                }
                else
                {
                    std::snprintf(metric, sizeof(metric), "%s DPS | %llu DMG | ACC %s", rate, static_cast<unsigned long long>(row.amount), row.accuracy.c_str());
                }
                const float metricWidth = ImGui::CalcTextSize(metric).x;
                const float nameWidth   = ImGui::CalcTextSize(row.name.c_str()).x;
                const float rowWidth    = ImGui::GetContentRegionAvail().x;
                ImGui::TextWrapped("%s", row.name.c_str());
                if (nameWidth + ImGui::GetStyle().ItemSpacing.x + metricWidth <= rowWidth)
                {
                    ImGui::SameLine();
                    ImGui::SetCursorPosX(std::max(ImGui::GetCursorPosX(),
                                                  ImGui::GetWindowContentRegionMax().x - metricWidth));
                    ImGui::TextColored(ImVec4(0.72f, 0.78f, 0.87f, 1.0f), "%s", metric);
                }
                else
                {
                    ImGui::TextColored(ImVec4(0.72f, 0.78f, 0.87f, 1.0f), "%s", metric);
                }

                const ImVec2 barOrigin = ImGui::GetCursorScreenPos();
                const float  barWidth  = ImGui::GetContentRegionAvail().x;
                const float  fillWidth = maximumRate > 0.0 && std::isfinite(row.rate)
                                             ? barWidth * static_cast<float>(std::clamp(row.rate / maximumRate, 0.0, 1.0))
                                             : 0.0f;
                ImDrawList*  draw      = ImGui::GetWindowDrawList();
                draw->AddRectFilled(barOrigin, ImVec2(barOrigin.x + barWidth, barOrigin.y + 6.0f), IM_COL32(42, 49, 62, 255), 3.0f);
                if (fillWidth > 0.0f)
                {
                    draw->AddRectFilled(barOrigin, ImVec2(barOrigin.x + fillWidth, barOrigin.y + 6.0f), ParseMeterColor(row.color), 3.0f);
                }
                ImGui::Dummy(ImVec2(barWidth, 6.0f));
                ImGui::Dummy(ImVec2(0.0f, 1.0f));
                ImGui::PopID();
            }
        }
        if (incomplete)
        {
            ImGui::TextColored(ImVec4(1.0f, 0.68f, 0.30f, 1.0f), "INCOMPLETE");
        }
    }
    const ImGuiIO& io = ImGui::GetIO();
    if (!locked && io.KeyShift && ImGui::IsWindowHovered(ImGuiHoveredFlags_AllowWhenBlockedByActiveItem) && ImGui::IsMouseClicked(ImGuiMouseButton_Left))
    {
        gDraggedAddonId = addonId;
    }
    if (locked && gDraggedAddonId == addonId)
    {
        gDraggedAddonId.clear();
    }
    else if (gDraggedAddonId == addonId)
    {
        if (!io.KeyShift || !ImGui::IsMouseDown(ImGuiMouseButton_Left))
        {
            gDraggedAddonId.clear();
            gSaveLayoutRequested = true;
        }
        else
        {
            const ImVec2 position = ImGui::GetWindowPos();
            ImGui::SetWindowPos(ImVec2(position.x + io.MouseDelta.x,
                                       position.y + io.MouseDelta.y));
        }
    }
    ImGui::End();
    ImGui::PopStyleColor();
}

void DrawZoneNamePopup(std::string_view text, float opacity)
{
    const auto             separator = text.find('\n');
    const std::string_view region    = separator == std::string_view::npos ? std::string_view{} : text.substr(0, separator);
    const std::string_view area      = separator == std::string_view::npos ? text : text.substr(separator + 1);
    const ImVec2           origin    = ImGui::GetWindowPos();
    const ImVec2           size      = ImGui::GetWindowSize();
    ImDrawList*            draw      = ImGui::GetWindowDrawList();
    const float            fade      = std::clamp(opacity, 0.0f, 1.0f);
    const auto             color     = [fade](int red, int green, int blue, int alpha)
    {
        return IM_COL32(red, green, blue, static_cast<int>(alpha * fade));
    };
    const float left      = origin.x;
    const float right     = origin.x + size.x;
    const float top       = origin.y;
    const float bottom    = origin.y + size.y;
    const float center    = left + size.x * 0.5f;
    const float fadeLeft  = left + size.x * 0.22f;
    const float fadeRight = left + size.x * 0.78f;
    draw->AddRectFilledMultiColor(ImVec2(left, top), ImVec2(fadeLeft, bottom), color(8, 10, 13, 0), color(8, 10, 13, 140), color(8, 10, 13, 140), color(8, 10, 13, 0));
    draw->AddRectFilledMultiColor(ImVec2(fadeLeft, top), ImVec2(center, bottom), color(8, 10, 13, 140), color(8, 10, 13, 176), color(8, 10, 13, 176), color(8, 10, 13, 140));
    draw->AddRectFilledMultiColor(ImVec2(center, top), ImVec2(fadeRight, bottom), color(8, 10, 13, 176), color(8, 10, 13, 140), color(8, 10, 13, 140), color(8, 10, 13, 176));
    draw->AddRectFilledMultiColor(ImVec2(fadeRight, top), ImVec2(right, bottom), color(8, 10, 13, 140), color(8, 10, 13, 0), color(8, 10, 13, 0), color(8, 10, 13, 140));

    const auto drawCentered = [&](std::string_view value, ImFont* font, float fontSize, float y, ImU32 foreground, float shadowDepth)
    {
        if (value.empty())
        {
            return;
        }
        font                  = font != nullptr ? font : ImGui::GetFont();
        const ImVec2 measured = font->CalcTextSizeA(fontSize, FLT_MAX, 0.0f, value.data(), value.data() + value.size());
        if (measured.x > size.x - 72.0f)
        {
            fontSize *= (size.x - 72.0f) / measured.x;
        }
        const ImVec2 textSize = font->CalcTextSizeA(fontSize, FLT_MAX, 0.0f, value.data(), value.data() + value.size());
        const ImVec2 position(left + (size.x - textSize.x) * 0.5f, top + y);
        draw->AddText(font, fontSize, ImVec2(position.x + shadowDepth, position.y + shadowDepth), color(0, 0, 0, 230), value.data(), value.data() + value.size());
        draw->AddText(font, fontSize, position, foreground, value.data(), value.data() + value.size());
    };
    drawCentered(region, gZoneRegionFont, 13.0f, 14.0f, color(231, 215, 173, 255), 1.0f);
    drawCentered(area, gZoneAreaFont, 30.0f, 37.0f, color(255, 247, 227, 255), 2.0f);

    const float ruleY     = top + 94.0f;
    const ImU32 ruleColor = color(216, 195, 143, 153);
    draw->AddLine(ImVec2(left + 36.0f, ruleY), ImVec2(center - 12.0f, ruleY), ruleColor);
    draw->AddLine(ImVec2(center + 12.0f, ruleY), ImVec2(right - 36.0f, ruleY), ruleColor);
    const ImVec2 diamond[] = {
        ImVec2(center, ruleY - 4.0f), ImVec2(center + 4.0f, ruleY), ImVec2(center, ruleY + 4.0f), ImVec2(center - 4.0f, ruleY)
    };
    draw->AddConvexPolyFilled(diamond, 4, color(216, 195, 143, 255));
}

void DrawAddonRawText(void* context, const char* addonId, const char* text, float red, float green, float blue, float alpha, bool locked, float fontSize)
{
    UNREFERENCED_PARAMETER(context);
    const ImGuiViewport*   viewport = ImGui::GetMainViewport();
    const std::string_view id(addonId);
    const bool             areaPopup = id == "zonename";
    if (areaPopup)
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x * 0.5f,
                                    viewport->WorkPos.y + 115.0f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(0.5f, 0.0f));
        ImGui::SetNextWindowSize(ImVec2(std::min(620.0f, viewport->WorkSize.x), 106.0f));
    }
    else if (id == "distance")
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x - 28.0f,
                                    viewport->WorkPos.y + viewport->WorkSize.y * 0.5f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(1.0f, 0.5f));
    }
    else if (id == "targethp")
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x * 0.25f,
                                    viewport->WorkPos.y + 16.0f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(0.5f, 0.0f));
    }
    else
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x - 8.0f,
                                    viewport->WorkPos.y + 8.0f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(1.0f, 0.0f));
    }
    const std::string windowName = id == "distance" || id == "targethp"
                                       ? "###bahamut-addon-" + std::string(addonId)
                                       : "###bahamut-addon-raw-" + std::string(addonId);
    ImGuiWindowFlags  flags      = ImGuiWindowFlags_NoDecoration | ImGuiWindowFlags_NoBackground | ImGuiWindowFlags_NoMove;
    if (!areaPopup)
    {
        flags |= ImGuiWindowFlags_AlwaysAutoResize;
    }
    if (locked || !ImGui::GetIO().KeyShift)
    {
        flags |= ImGuiWindowFlags_NoInputs;
    }
    if (ImGui::Begin(windowName.c_str(), nullptr, flags))
    {
        ImGui::SetWindowFontScale(fontSize > 0.0F ? fontSize / ImGui::GetFont()->FontSize : 1.0F);
        if (areaPopup)
        {
            DrawZoneNamePopup(text, alpha);
        }
        else
        {
            ImGui::TextColored(ImVec4(red, green, blue, alpha), "%s", text);
        }
    }
    const ImGuiIO& io = ImGui::GetIO();
    if (!locked && io.KeyShift && ImGui::IsWindowHovered(ImGuiHoveredFlags_AllowWhenBlockedByActiveItem) && ImGui::IsMouseClicked(ImGuiMouseButton_Left))
    {
        gDraggedAddonId = addonId;
    }
    if (locked && gDraggedAddonId == addonId)
    {
        gDraggedAddonId.clear();
    }
    else if (gDraggedAddonId == addonId)
    {
        if (!io.KeyShift || !ImGui::IsMouseDown(ImGuiMouseButton_Left))
        {
            gDraggedAddonId.clear();
            gSaveLayoutRequested = true;
        }
        else
        {
            const ImVec2 position = ImGui::GetWindowPos();
            ImGui::SetWindowPos(ImVec2(
                position.x + io.MouseDelta.x, position.y + io.MouseDelta.y));
        }
    }
    ImGui::End();
}

void DrawAddonTargetlines(void* context, const char* addonId)
{
    UNREFERENCED_PARAMETER(context);
    if (std::string_view(addonId) != "targetlines")
    {
        return;
    }
    const auto targetlines = bahamut_client::TargetlinesFrame();
    const auto displaySize = ImGui::GetIO().DisplaySize;
    // Projection uses back-buffer pixels; draw only at a matching display size.
    if (targetlines && displaySize.x == static_cast<float>(targetlines->backBufferWidth) &&
        displaySize.y == static_cast<float>(targetlines->backBufferHeight))
    {
        auto* draw = ImGui::GetBackgroundDrawList();
        for (std::size_t arcIndex = 0; arcIndex < targetlines->count; ++arcIndex)
        {
            const auto& arc   = targetlines->arcs[arcIndex];
            const int   red   = arc.friendly ? 42 : 255;
            const int   green = arc.friendly ? 255 : 48;
            const int   blue  = arc.friendly ? 112 : 72;
            const auto  color = [&](int alpha)
            {
                return IM_COL32(red, green, blue, static_cast<int>(alpha * arc.opacity));
            };
            // Layered strokes provide a soft beam without a texture asset.
            for (std::size_t pieceIndex = 0; pieceIndex < arc.pieces.count; ++pieceIndex)
            {
                const auto&  piece = arc.pieces.segments[pieceIndex];
                const ImVec2 source{ piece.source.x, piece.source.y };
                const ImVec2 target{ piece.target.x, piece.target.y };
                draw->AddLine(source, target, color(28), 7.0F);
                draw->AddLine(source, target, color(76), 4.0F);
                draw->AddLine(source, target, color(230), 1.8F);
                draw->AddLine(source, target, IM_COL32(255, 240, 238, static_cast<int>(170.0F * arc.opacity)), 0.65F);
            }
        }
    }
}

void RecordFillModeTestResult(IDirect3DDevice9* device)
{
    if (!IsWireframeEnabled() || InterlockedCompareExchange(&gFillModeTestRecorded, 1, 0) != 0)
    {
        return;
    }
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_TEST_OVERLAY_FILL_MODE_RESULT_FILE", path, ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    DWORD         fillMode = 0;
    const bool    solid    = SUCCEEDED(device->GetRenderState(D3DRS_FILLMODE, &fillMode)) && fillMode == D3DFILL_SOLID;
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << "overlay_fill_mode=" << (solid ? "solid" : "other") << "\n";
    }
}

// Managed-pool icons survive Reset; failed loads cache nullptr.
std::unordered_map<std::uint32_t, IDirect3DTexture9*> gIconTextures;

void ReleaseIconTextures()
{
    for (auto& entry : gIconTextures)
    {
        if (entry.second != nullptr)
        {
            entry.second->Release();
        }
    }
    gIconTextures.clear();
}

IDirect3DTexture9* CreateIconTexture(IDirect3DDevice9* device, std::uint32_t icon)
{
    const auto relative = bahamut_client::xhbar::IconResourcePath(icon);
    wchar_t    module[MAX_PATH]{};
    const auto length = GetModuleFileNameW(nullptr, module, MAX_PATH);
    if (!relative || length == 0 || length >= MAX_PATH)
    {
        return nullptr;
    }
    std::ifstream              file(std::filesystem::path(module).parent_path() / *relative, std::ios::binary);
    std::vector<unsigned char> bytes((std::istreambuf_iterator<char>(file)), std::istreambuf_iterator<char>());
    const auto                 image = bahamut_client::xhbar::ParseIcon(bytes.data(), bytes.size());
    if (!image)
    {
        return nullptr;
    }
    D3DFORMAT     format    = D3DFMT_DXT5;
    std::uint32_t rows      = image->height / 4;
    std::size_t   rowLength = static_cast<std::size_t>(image->width / 4) * 16u;
    if (image->format == bahamut_client::xhbar::IconFormat::Dxt1)
    {
        format    = D3DFMT_DXT1;
        rowLength = static_cast<std::size_t>(image->width / 4) * 8u;
    }
    else if (image->format == bahamut_client::xhbar::IconFormat::Argb8)
    {
        format    = D3DFMT_A8R8G8B8;
        rows      = image->height;
        rowLength = static_cast<std::size_t>(image->width) * 4u;
    }
    IDirect3DTexture9* texture = nullptr;
    if (FAILED(device->CreateTexture(image->width, image->height, 1, 0, format, D3DPOOL_MANAGED, &texture, nullptr)))
    {
        return nullptr;
    }
    D3DLOCKED_RECT locked{};
    if (FAILED(texture->LockRect(0, &locked, nullptr, 0)))
    {
        texture->Release();
        return nullptr;
    }
    for (std::uint32_t row = 0; row < rows; ++row)
    {
        std::memcpy(static_cast<unsigned char*>(locked.pBits) + static_cast<std::size_t>(row) * locked.Pitch, image->data + row * rowLength, rowLength);
    }
    texture->UnlockRect(0);
    return texture;
}

IDirect3DTexture9* IconTexture(IDirect3DDevice9* device, std::uint32_t icon)
{
    if (icon == 0 || device == nullptr)
    {
        return nullptr;
    }
    const auto found = gIconTextures.find(icon);
    if (found != gIconTextures.end())
    {
        return found->second;
    }
    IDirect3DTexture9* texture = CreateIconTexture(device, icon);
    gIconTextures.emplace(icon, texture);
    return texture;
}

bool DrawXHBar(IDirect3DDevice9* device)
{
    using bahamut_client::xhbar::Side;
    if (!bahamut_client::xhbar::BarAvailable())
    {
        return false;
    }
    const Side held = bahamut_client::xhbar::ActiveSide();

    constexpr float         kSlot       = 44.0f;
    constexpr float         kGap        = 4.0f;
    constexpr float         kCross      = kSlot * 3.0f + kGap * 2.0f;
    constexpr float         kCrossGap   = 16.0f;
    constexpr float         kSideGap    = 28.0f;
    constexpr float         kPadding    = 12.0f;
    constexpr float         kHeader     = 24.0f;
    constexpr float         kSide       = kCross * 2.0f + kCrossGap;
    constexpr float         kWidth      = kPadding * 2.0f + kSide * 2.0f + kSideGap;
    constexpr float         kHeight     = kPadding * 2.0f + kHeader + kSlot * 2.0f + kGap;
    constexpr float         kDimmed     = 0.42f;
    constexpr std::uint64_t kPressFlash = 220;

    // Cells follow Up, Down, Left, Right, A, B, X, Y; columns 0-2, rows 0/0.5/1.
    struct Cell
    {
        int   cross;
        float column;
        float row;
    };

    static constexpr std::array<Cell, 8> kCells = {
        Cell{ 0, 1.0f, 0.0f }, Cell{ 0, 1.0f, 1.0f }, Cell{ 0, 0.0f, 0.5f }, Cell{ 0, 2.0f, 0.5f }, Cell{ 1, 1.0f, 1.0f }, Cell{ 1, 2.0f, 0.5f }, Cell{ 1, 0.0f, 0.5f }, Cell{ 1, 1.0f, 0.0f }
    };

    struct FaceGlyph
    {
        const char* letter;
        int         red;
        int         green;
        int         blue;
    };

    static constexpr std::array<FaceGlyph, 4> kFaces = {
        FaceGlyph{ "A", 96, 176, 96 }, FaceGlyph{ "B", 204, 82, 82 }, FaceGlyph{ "X", 82, 128, 210 }, FaceGlyph{ "Y", 224, 192, 72 }
    };
    static constexpr std::array<FaceGlyph, 4> kShapes = {
        FaceGlyph{ "", 124, 150, 228 }, FaceGlyph{ "", 226, 92, 100 }, FaceGlyph{ "", 214, 136, 196 }, FaceGlyph{ "", 72, 196, 164 }
    };
    const bool                             playStation = bahamut_client::xhbar::Labels() == bahamut_client::xhbar::LabelStyle::PlayStation;
    static constexpr std::array<ImVec2, 4> kDirections = { ImVec2(0, -1), ImVec2(0, 1), ImVec2(-1, 0), ImVec2(1, 0) };

    const ImGuiViewport* viewport = ImGui::GetMainViewport();
    ImGui::SetNextWindowPos(ImVec2(viewport->WorkPos.x + viewport->WorkSize.x * 0.5f, viewport->WorkPos.y + viewport->WorkSize.y - 96.0f),
                            ImGuiCond_Always,
                            ImVec2(0.5f, 1.0f));
    ImGui::SetNextWindowSize(ImVec2(kWidth, kHeight), ImGuiCond_Always);
    const ImGuiWindowFlags flags = ImGuiWindowFlags_NoDecoration | ImGuiWindowFlags_NoBackground | ImGuiWindowFlags_NoInputs |
                                   ImGuiWindowFlags_NoSavedSettings | ImGuiWindowFlags_NoFocusOnAppearing | ImGuiWindowFlags_NoNav;
    ImGui::PushStyleVar(ImGuiStyleVar_WindowPadding, ImVec2(0.0f, 0.0f));
    ImGui::PushStyleVar(ImGuiStyleVar_WindowBorderSize, 0.0f);
    const bool visible = ImGui::Begin("###bahamut-xhbar", nullptr, flags);
    ImGui::PopStyleVar(2);
    if (!visible)
    {
        ImGui::End();
        return false;
    }

    ImDrawList*  draw   = ImGui::GetWindowDrawList();
    ImFont*      font   = ImGui::GetFont();
    const ImVec2 origin = ImGui::GetWindowPos();
    const auto   color  = [](int red, int green, int blue, float alpha)
    {
        return IM_COL32(red, green, blue, static_cast<int>(std::clamp(alpha, 0.0f, 1.0f) * 255.0f));
    };
    const auto centeredText = [&](float size, ImVec2 center, ImU32 fill, float alpha, const char* text)
    {
        const ImVec2 measured = font->CalcTextSizeA(size, FLT_MAX, 0.0f, text);
        const ImVec2 position(center.x - measured.x * 0.5f, center.y - measured.y * 0.5f);
        const ImU32  edge = color(38, 38, 38, alpha);
        for (const ImVec2 offset : { ImVec2(-1, 0), ImVec2(1, 0), ImVec2(0, -1), ImVec2(0, 1) })
        {
            draw->AddText(font, size, ImVec2(position.x + offset.x, position.y + offset.y), edge, text);
        }
        draw->AddText(font, size, position, fill, text);
    };

    const ImVec2 panelMax(origin.x + kWidth, origin.y + kHeight);
    draw->AddRectFilledMultiColor(origin, panelMax, color(0, 0, 51, 0.5f), color(0, 0, 51, 0.5f), color(0, 0, 51, 0.92f), color(0, 0, 51, 0.92f));
    draw->AddRect(origin, panelMax, color(184, 188, 198, 1.0f), 5.0f, 0, 1.5f);
    draw->AddRect(ImVec2(origin.x + 3.0f, origin.y + 3.0f), ImVec2(panelMax.x - 3.0f, panelMax.y - 3.0f), color(92, 96, 107, 1.0f), 3.0f, 0, 1.0f);
    const float dividerX = origin.x + kPadding + kSide + kSideGap * 0.5f;
    draw->AddLine(ImVec2(dividerX, origin.y + kPadding + 4.0f), ImVec2(dividerX, panelMax.y - kPadding - 4.0f), color(184, 188, 198, 0.35f), 1.0f);

    const std::uint64_t now = GetTickCount64();
    for (int sideIndex = 0; sideIndex < 2; ++sideIndex)
    {
        const bool        combo  = held == (sideIndex == 0 ? Side::LeftThenRight : Side::RightThenLeft);
        const bool        active = combo || held == (sideIndex == 0 ? Side::Left : Side::Right);
        const std::size_t base   = sideIndex == 0 ? (combo ? 24u : 0u) : (combo ? 16u : 8u);
        const float       alpha  = active || held == Side::None ? 1.0f : kDimmed;
        const float       sideX  = origin.x + kPadding + static_cast<float>(sideIndex) * (kSide + kSideGap);
        const float       crossY = origin.y + kPadding + kHeader;

        const ImVec2 tabCenter(sideX + kSide * 0.5f, origin.y + kPadding + kHeader * 0.4f);
        const ImVec2 tabMin(tabCenter.x - 24.0f, tabCenter.y - 10.0f);
        const ImVec2 tabMax(tabCenter.x + 24.0f, tabCenter.y + 10.0f);
        draw->AddRectFilled(tabMin, tabMax, color(26, 26, 68, alpha), 10.0f);
        draw->AddRect(tabMin, tabMax, active ? color(232, 210, 122, 1.0f) : color(92, 96, 107, alpha), 10.0f, 0, 1.0f);
        const char* tab = playStation ? (sideIndex == 0 ? (combo ? "L2>R2" : "L2") : (combo ? "R2>L2" : "R2"))
                                      : (sideIndex == 0 ? (combo ? "LT>RT" : "LT") : (combo ? "RT>LT" : "RT"));
        centeredText(14.0f, tabCenter, color(243, 243, 243, alpha), alpha, tab);

        for (std::size_t index = 0; index < kCells.size(); ++index)
        {
            const Cell        cell   = kCells[index];
            const std::size_t slot   = base + index;
            const auto        target = bahamut_client::xhbar::Shortcut(slot);
            const int         number = target.page == 0 ? 0 : target.button;
            const ImVec2      slotMin(sideX + static_cast<float>(cell.cross) * (kCross + kCrossGap) + cell.column * (kSlot + kGap), crossY + cell.row * (kSlot + kGap));
            const ImVec2      slotMax(slotMin.x + kSlot, slotMin.y + kSlot);
            const ImVec2      center(slotMin.x + kSlot * 0.5f, slotMin.y + kSlot * 0.5f);

            draw->AddRectFilled(slotMin, slotMax, number == 0 ? color(16, 16, 40, alpha * 0.8f) : color(30, 30, 70, alpha), 4.0f);
            draw->AddRectFilledMultiColor(ImVec2(slotMin.x + 2.0f, slotMin.y + 2.0f),
                                          ImVec2(slotMax.x - 2.0f, slotMin.y + kSlot * 0.5f),
                                          color(255, 255, 255, alpha * 0.10f),
                                          color(255, 255, 255, alpha * 0.10f),
                                          color(255, 255, 255, 0.0f),
                                          color(255, 255, 255, 0.0f));
            draw->AddRect(slotMin, slotMax, number == 0 ? color(70, 74, 86, alpha) : color(141, 147, 163, alpha), 4.0f, 0, 1.0f);
            const auto         view = bahamut_client::xhbar::SlotDisplay(slot);
            IDirect3DTexture9* icon = number == 0 ? nullptr : IconTexture(device, view.icon);
            // Match the native bar's disabled-slot greying.
            const bool unavailable = view.remaining > 0 || view.incompatible;
            const int  shade       = unavailable ? 110 : 255;
            if (icon != nullptr)
            {
                draw->AddImageRounded(static_cast<ImTextureID>(reinterpret_cast<std::uintptr_t>(icon)),
                                      ImVec2(slotMin.x + 3.0f, slotMin.y + 3.0f),
                                      ImVec2(slotMax.x - 3.0f, slotMax.y - 3.0f),
                                      ImVec2(0.0f, 0.0f),
                                      ImVec2(1.0f, 1.0f),
                                      color(shade, shade, shade, alpha),
                                      3.0f);
                char label[4]{};
                std::snprintf(label, sizeof(label), "%d", number);
                centeredText(12.0f, ImVec2(slotMin.x + 9.0f, slotMin.y + 9.0f), color(249, 240, 122, alpha), alpha, label);
            }
            else if (number != 0)
            {
                char label[4]{};
                std::snprintf(label, sizeof(label), "%d", number);
                centeredText(20.0f, ImVec2(center.x - 2.0f, center.y - 2.0f), color(249, 240, 122, alpha), alpha, label);
            }

            if (view.remaining > 0)
            {
                draw->AddRectFilled(slotMin, slotMax, color(0, 0, 0, alpha * 0.45f), 4.0f);
                char seconds[8]{};
                std::snprintf(seconds, sizeof(seconds), "%d", view.remaining);
                centeredText(18.0f, center, color(255, 255, 255, alpha), alpha, seconds);
            }

            if (view.page != 0)
            {
                char marker[8]{};
                std::snprintf(marker, sizeof(marker), "P%d", view.page);
                centeredText(11.0f, ImVec2(slotMax.x - 10.0f, slotMin.y + 8.0f), color(200, 206, 222, alpha), alpha, marker);
            }

            const ImVec2 glyph(slotMax.x - 9.0f, slotMax.y - 9.0f);
            if (cell.cross == 0)
            {
                const ImVec2 forward = kDirections[index];
                const ImVec2 across(-forward.y, forward.x);
                draw->AddCircleFilled(glyph, 6.5f, color(20, 20, 30, alpha));
                draw->AddTriangleFilled(ImVec2(glyph.x + forward.x * 4.0f, glyph.y + forward.y * 4.0f),
                                        ImVec2(glyph.x - forward.x * 2.5f + across.x * 3.5f, glyph.y - forward.y * 2.5f + across.y * 3.5f),
                                        ImVec2(glyph.x - forward.x * 2.5f - across.x * 3.5f, glyph.y - forward.y * 2.5f - across.y * 3.5f),
                                        color(220, 222, 228, alpha));
            }
            else
            {
                const FaceGlyph face = (playStation ? kShapes : kFaces)[index - 4];
                const ImU32     tint = color(face.red, face.green, face.blue, alpha);
                draw->AddCircleFilled(glyph, 7.0f, color(20, 20, 30, alpha));
                draw->AddCircle(glyph, 7.0f, tint, 0, 1.5f);
                if (!playStation)
                    centeredText(11.0f, glyph, tint, alpha, face.letter);
                else if (index == 4)
                {
                    draw->AddLine(ImVec2(glyph.x - 3.0f, glyph.y - 3.0f), ImVec2(glyph.x + 3.0f, glyph.y + 3.0f), tint, 1.5f);
                    draw->AddLine(ImVec2(glyph.x - 3.0f, glyph.y + 3.0f), ImVec2(glyph.x + 3.0f, glyph.y - 3.0f), tint, 1.5f);
                }
                else if (index == 5)
                    draw->AddCircle(glyph, 3.5f, tint, 0, 1.5f);
                else if (index == 6)
                    draw->AddRect(ImVec2(glyph.x - 3.0f, glyph.y - 3.0f), ImVec2(glyph.x + 3.0f, glyph.y + 3.0f), tint, 0.0f, 0, 1.5f);
                else
                    draw->AddTriangle(ImVec2(glyph.x, glyph.y - 3.5f), ImVec2(glyph.x + 3.5f, glyph.y + 2.5f), ImVec2(glyph.x - 3.5f, glyph.y + 2.5f), tint, 1.5f);
            }

            const std::uint64_t fired = bahamut_client::xhbar::LastDispatchTick(slot);
            if (fired != 0 && now - fired < kPressFlash)
            {
                const float fade = 1.0f - static_cast<float>(now - fired) / static_cast<float>(kPressFlash);
                draw->AddRect(ImVec2(slotMin.x - 2.0f, slotMin.y - 2.0f), ImVec2(slotMax.x + 2.0f, slotMax.y + 2.0f), color(232, 210, 122, fade), 5.0f, 0, 2.0f);
                draw->AddRectFilled(slotMin, slotMax, color(255, 244, 200, fade * 0.18f), 4.0f);
            }
        }
    }

    ImGui::End();
    return true;
}

} // namespace

void ConfigureOverlayWindow(HWND window)
{
    gWindow = window;
}

void ConfigureOverlayLayout(const std::filesystem::path& path)
{
    OverlayLayoutPath() = path;
}

bool BindOverlayDevice(IDirect3DDevice9* device)
{
    if (device == nullptr)
    {
        return false;
    }
    if (gOverlayDevice == nullptr)
    {
        gOverlayDevice = device;
    }
    return gOverlayDevice == device;
}

void UnbindOverlayDevice(IDirect3DDevice9* device)
{
    if (gOverlayDevice == device)
    {
        ReleaseIconTextures();
        gOverlayDevice = nullptr;
        SetOverlayMenuInput(false, false);
    }
}

bool IsOverlayDevice(IDirect3DDevice9* device)
{
    return device != nullptr && device == gOverlayDevice;
}

bool IsOverlayDrawing(IDirect3DDevice9* device)
{
    return IsOverlayDevice(device) && InterlockedCompareExchange(&gOverlayDrawing, 0, 0) != 0;
}

void DrawOverlay(IDirect3DDevice9*                   device,
                 BahamutRuntimeTelemetry*            telemetry,
                 AddonHost*                          addonHost,
                 NativePluginHost*                   discordPluginHost,
                 bahamut_client::PlayerStateService* playerState)
{
    const HRESULT cooperativeLevel = device->TestCooperativeLevel();
    if (cooperativeLevel == D3DERR_DEVICELOST || cooperativeLevel == D3DERR_DEVICENOTRESET)
    {
        SetOverlayMenuInput(false, false);
        return;
    }
    if (!gInitialized && !InitializeOverlay(device))
    {
        ++telemetry->overlayFailureCount;
        SetOverlayMenuInput(false, false);
        return;
    }
    if (!gBackendReady && !CompleteOverlayReset())
    {
        ++telemetry->overlayFailureCount;
        SetOverlayMenuInput(false, false);
        return;
    }
    UpdateControllerObservation();
    bahamut_client::StepAutologin();

    if (discordPluginHost != nullptr && playerState != nullptr)
    {
        BahamutNativePluginPlayerStateV1 state{};
        state.structSize    = sizeof(state);
        const auto snapshot = playerState->Snapshot();
        if (snapshot.has_value())
        {
            state.actorId     = snapshot->actorId;
            state.zoneId      = snapshot->zoneId;
            state.baseClassId = snapshot->baseClassId;
            state.jobId       = snapshot->jobId;
            state.level       = snapshot->level;
            state.flags       = BahamutNativePluginPlayerStateHasCharacter;
            if (!snapshot->areaName.empty())
            {
                state.flags |= BahamutNativePluginPlayerStateHasArea;
            }
            if ((snapshot->baseClassId != 0u || snapshot->jobId != 0u) &&
                snapshot->level != 0u)
            {
                state.flags |= BahamutNativePluginPlayerStateHasClassJob;
            }
            const auto copyText = [](char*              destination,
                                     std::size_t        capacity,
                                     const std::string& source)
            {
                const std::size_t count = std::min(capacity - 1u, source.size());
                std::memcpy(destination, source.data(), count);
                destination[count] = '\0';
            };
            copyText(state.displayName, sizeof(state.displayName), snapshot->displayName);
            copyText(state.areaName, sizeof(state.areaName), snapshot->areaName);
        }
        static_cast<void>(discordPluginHost->PublishPlayerState(state));
    }

    LARGE_INTEGER now{};
    LARGE_INTEGER frequency{};
    QueryPerformanceCounter(&now);
    QueryPerformanceFrequency(&frequency);
    const double frameDelta = gLastAddonFrame.QuadPart == 0 || frequency.QuadPart == 0
                                  ? 0.0
                                  : static_cast<double>(now.QuadPart - gLastAddonFrame.QuadPart) / static_cast<double>(frequency.QuadPart);
    gLastAddonFrame         = now;
    if (addonHost != nullptr)
    {
        addonHost->Update(frameDelta);
    }
    if (!IsOverlayVisible())
    {
        SetOverlayMenuInput(false, false);
        return;
    }

    BahamutRuntimeTimingSnapshot timing{};
    if (!BahamutReadTimingSnapshot(*telemetry, timing))
    {
        ++telemetry->overlayFailureCount;
        SetOverlayMenuInput(false, false);
        return;
    }

    LARGE_INTEGER start{};
    QueryPerformanceCounter(&start);
    CompleteDeviceState state(device);
    if (!state.IsCaptured() || !state.BindBackBuffer())
    {
        ++telemetry->overlayFailureCount;
        SetOverlayMenuInput(false, false);
        return;
    }
    RecordBackBufferTestResult(state);
    ImGui_ImplDX9_NewFrame();
    ImGui_ImplWin32_NewFrame();
    ImGui::NewFrame();
    gMenuMouseFrame    = false;
    gMenuKeyboardFrame = false;
    if (addonHost != nullptr)
    {
        gNextAddonWindowY = ImGui::GetMainViewport()->WorkPos.y + 16.0f;
        addonHost->Draw({ nullptr, &DrawAddonWindow, &DrawAddonRawText, &DrawAddonCombatMeter, &DrawAddonTargetlines, &DrawGearsets });
    }
    SetOverlayMenuInput(gMenuMouseFrame, gMenuKeyboardFrame);
    const bool xhbar = DrawXHBar(device);
    InterlockedExchange(&gOverlayDrawing, 1);
    device->SetRenderState(D3DRS_FILLMODE, D3DFILL_SOLID);
    ImGui::Render();
    ImGui_ImplDX9_RenderDrawData(ImGui::GetDrawData());
    if (xhbar)
    {
        bahamut_client::xhbar::MarkBarDrawn();
    }
    if (gSaveLayoutRequested || ImGui::GetIO().WantSaveIniSettings)
    {
        SaveOverlayLayout();
        gSaveLayoutRequested               = false;
        ImGui::GetIO().WantSaveIniSettings = false;
    }
    RecordFillModeTestResult(device);
    InterlockedExchange(&gOverlayDrawing, 0);

    const bool    restored = state.Restore();
    LARGE_INTEGER end{};
    QueryPerformanceCounter(&end);
    const LONGLONG elapsed = end.QuadPart - start.QuadPart;
    BahamutBeginTimingWrite(telemetry);
    telemetry->overlayTotalCounter.QuadPart += elapsed;
    telemetry->overlayMaxCounter.QuadPart = std::max(
        timing.overlayMaxCounter.QuadPart, elapsed);
    ++telemetry->overlayFrameCount;
    BahamutEndTimingWrite(telemetry);
    if (!restored)
    {
        ++telemetry->overlayFailureCount;
    }
}

void PrepareOverlayReset()
{
    gLastAddonFrame = {};
    if (gInitialized)
    {
        gBackendReady = false;
        ImGui_ImplDX9_InvalidateDeviceObjects();
    }
}

bool CompleteOverlayReset()
{
    if (!gInitialized)
    {
        return true;
    }
    if (HasTestFault(L"BAHAMUT_TEST_OVERLAY_BACKEND_RETRY"))
    {
        const LONG attempt = ++gBackendRetryAttempt;
        if (attempt == 1)
        {
            gBackendReady = false;
            RecordTestResult("backend_retry=first_failed\n");
            return false;
        }
    }
    gBackendReady = ImGui_ImplDX9_CreateDeviceObjects();
    if (gBackendReady && HasTestFault(L"BAHAMUT_TEST_OVERLAY_BACKEND_RETRY") && gBackendRetryAttempt >= 2)
    {
        RecordTestResult("backend_retry=ok\n");
    }
    return gBackendReady;
}
