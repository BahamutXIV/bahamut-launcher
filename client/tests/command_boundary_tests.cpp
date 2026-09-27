#include "chat_boundary.h"
#include "command_boundary.h"

#include <algorithm>
#include <array>
#include <cstring>
#include <initializer_list>
#include <iostream>
#include <string>
#include <string_view>
#include <utility>

namespace
{

struct DispatchState
{
    bool        handled        = false;
    bool        throwException = false;
    int         calls          = 0;
    std::string lastCommand;
};

struct OriginalState
{
    int lookupCalls  = 0;
    int lookupResult = 42;
};

OriginalState* gOriginalState = nullptr;

struct TokenElement
{
    const char*                                           text = nullptr;
    std::array<unsigned char, 0x54 - sizeof(const char*)> remainder{};
};

static_assert(sizeof(void*) == 4);
static_assert(sizeof(TokenElement) == 0x54);

bool Dispatch(void* context, std::string_view command)
{
    auto& state = *static_cast<DispatchState*>(context);
    ++state.calls;
    state.lastCommand.assign(command.data(), command.size());
    if (state.throwException)
    {
        throw 1;
    }
    return state.handled;
}

int OriginalLookup(void*, const void*, void* output)
{
    ++gOriginalState->lookupCalls;
    std::memset(output, 0xA5, 16);
    return gOriginalState->lookupResult;
}

bool Require(bool condition, const char* message)
{
    if (!condition)
    {
        std::cerr << message << '\n';
        return false;
    }
    return true;
}

} // namespace

int wmain()
{
    const char                      capturedText[] = "hello";
    std::array<unsigned char, 0x54> capturedString{};
    const char*                     capturedData = capturedText;
    const std::uint32_t             capturedSize = sizeof(capturedText);
    std::memcpy(capturedString.data(), &capturedData, sizeof(capturedData));
    std::memcpy(capturedString.data() + 0x08, &capturedSize, sizeof(capturedSize));
    std::string capturedCopy;
    if (!Require(CopyRetailChatText(capturedString.data(), capturedCopy) && capturedCopy == "hello",
                 "chat capture did not use the NUL-inclusive retail size"))
    {
        return 1;
    }
    const std::uint32_t emptySize = 0;
    std::memcpy(capturedString.data() + 0x08, &emptySize, sizeof(emptySize));
    if (!Require(!CopyRetailChatText(capturedString.data(), capturedCopy),
                 "chat capture accepted an invalid retail size"))
    {
        return 1;
    }

    const std::array<std::pair<std::string_view, bool>, 30> recognition = { {
        { "/pos", true },
        { "/pos ", true },
        { "/pos\targ", true },
        { "/pos\narg", true },
        { "/positional", false },
        { "/poss", false },
        { "pos", false },
        { "/po", false },
        { "/pos\x01", false },
        { "/fps", true },
        { "/fps ", true },
        { "/fpscounter", false },
        { "/wiki", true },
        { "/wiki ", true },
        { "/wikilist", false },
        { "/wiki\x01", false },
        { "/distance", true },
        { "/distance lock", true },
        { "/distances", false },
        { "/targethp", true },
        { "/targethps", false },
        { "/packetlogger", true },
        { "/packetlogger start", true },
        { "/packetloggers", false },
        { "/combatparser reset", true },
        { "/combatparser", true },
        { "/combatparsers", false },
    } };
    for (const auto& [input, expected] : recognition)
    {
        TokenElement token;
        token.text = input.data();
        if (!Require(CommandBoundary::RecognizesAddonCommand(&token) == expected,
                     "command token boundary recognition failed"))
        {
            return 1;
        }
    }

    const std::array<unsigned char, 40> lookupSignature = {
        0x6A,
        0xFF,
        0x68,
        0x74,
        0xF6,
        0xE6,
        0x00,
        0x64,
        0xA1,
        0x00,
        0x00,
        0x00,
        0x00,
        0x50,
        0x83,
        0xEC,
        0x54,
        0x53,
        0x56,
        0x57,
        0xA1,
        0xB0,
        0xA8,
        0x2E,
        0x01,
        0x33,
        0xC4,
        0x50,
        0x8D,
        0x44,
        0x24,
        0x64,
        0x64,
        0xA3,
        0x00,
        0x00,
        0x00,
        0x00,
        0x8B,
        0xF1,
    };
    std::array<unsigned char, 40> alteredLookupSignature = lookupSignature;
    alteredLookupSignature[0] ^= 0x01;
    if (!Require(CommandBoundaryLookupSignatureMatches(lookupSignature.data()) && !CommandBoundaryLookupSignatureMatches(
                                                                                      alteredLookupSignature.data()),
                 "lookup signature gate did not require exact target bytes"))
    {
        return 1;
    }
    DispatchState dispatch;
    OriginalState originalState;
    gOriginalState = &originalState;
    CommandBoundary               boundary(&dispatch, &Dispatch);
    std::array<unsigned char, 16> output;
    output.fill(0x5A);
    auto handleLookup = [&](std::initializer_list<const char*> tokenTexts)
    {
        std::array<TokenElement, 4> tokens{};
        std::size_t                 count = 0;
        for (const char* text : tokenTexts)
        {
            tokens[count++].text = text;
        }
        const auto* end = reinterpret_cast<const unsigned char*>(tokens.data()) + count * sizeof(TokenElement);
        return boundary.HandleLookupForTest(&OriginalLookup, nullptr, tokens.data(), output.data(), true, tokens.data(), end);
    };

    dispatch.handled = true;
    if (!Require(handleLookup({ "/pos" }) == -2,
                 "handled lookup did not synthesize a miss") ||
        !Require(originalState.lookupCalls == 0,
                 "handled lookup forwarded to retail") ||
        !Require(std::all_of(output.begin(), output.end(), [](unsigned char value)
                             {
                                 return value == 0x5A;
                             }),
                 "handled lookup modified retail output") ||
        !Require(dispatch.calls == 1 && dispatch.lastCommand == "/pos", "handled lookup dispatch was not exact"))
    {
        return 1;
    }
    const CommandBoundarySnapshot tokenHandled = boundary.Snapshot();
    if (!Require(tokenHandled.lookupRecognized == 1,
                 "token recognition diagnostic was not recorded"))
    {
        return 1;
    }

    boundary.ResetDiagnostics();
    dispatch.calls = 0;
    if (!Require(handleLookup({ "/pos" }) == -2 && dispatch.lastCommand == "/pos",
                 "base command was not dispatched") ||
        !Require(handleLookup({ "/pos", "help" }) == -2 && dispatch.lastCommand == "/pos help",
                 "help command was not dispatched") ||
        !Require(handleLookup({ "/fps" }) == -2 && dispatch.lastCommand == "/fps",
                 "fps command was not dispatched") ||
        !Require(handleLookup({ "/fps", "help" }) == -2 && dispatch.lastCommand == "/fps help",
                 "fps help command was not dispatched") ||
        !Require(handleLookup({ "/fps", "lock" }) == -2 && dispatch.lastCommand == "/fps lock",
                 "fps lock command was not dispatched") ||
        !Require(handleLookup({ "/fps", "color", "#00FF80" }) == -2 && dispatch.lastCommand == "/fps color #00FF80",
                 "fps color command was not dispatched") ||
        !Require(handleLookup({ "/fps", "size", "24" }) == -2 && dispatch.lastCommand == "/fps size 24",
                 "fps size command was not dispatched") ||
        !Require(handleLookup({ "/pos", "lock" }) == -2 && dispatch.lastCommand == "/pos lock",
                 "pos lock command was not dispatched") ||
        !Require(handleLookup({ "/distance", "lock" }) == -2 && dispatch.lastCommand == "/distance lock",
                 "distance lock command was not dispatched") ||
        !Require(handleLookup({ "/distance", "help" }) == -2 && dispatch.lastCommand == "/distance help",
                 "distance help command was not dispatched") ||
        !Require(handleLookup({ "/distance", "color", "#00FF80" }) == -2 && dispatch.lastCommand == "/distance color #00FF80",
                 "distance color command was not dispatched") ||
        !Require(handleLookup({ "/distance", "size", "24" }) == -2 && dispatch.lastCommand == "/distance size 24",
                 "distance size command was not dispatched") ||
        !Require(handleLookup({ "/targethp", "lock" }) == -2 && dispatch.lastCommand == "/targethp lock",
                 "targethp lock command was not dispatched") ||
        !Require(handleLookup({ "/targethp", "help" }) == -2 && dispatch.lastCommand == "/targethp help",
                 "targethp help command was not dispatched") ||
        !Require(handleLookup({ "/targethp", "color", "#00FF80" }) == -2 && dispatch.lastCommand == "/targethp color #00FF80",
                 "targethp color command was not dispatched") ||
        !Require(handleLookup({ "/targethp", "size", "24" }) == -2 && dispatch.lastCommand == "/targethp size 24",
                 "targethp size command was not dispatched") ||
        !Require(handleLookup({ "/packetlogger" }) == -2 && dispatch.lastCommand == "/packetlogger",
                 "packetlogger status command was not dispatched") ||
        !Require(handleLookup({ "/packetlogger", "start" }) == -2 && dispatch.lastCommand == "/packetlogger start",
                 "packetlogger start command was not dispatched") ||
        !Require(handleLookup({ "/packetlogger", "stop" }) == -2 && dispatch.lastCommand == "/packetlogger stop",
                 "packetlogger stop command was not dispatched") ||
        !Require(handleLookup({ "/packetlogger", "status" }) == -2 && dispatch.lastCommand == "/packetlogger status",
                 "packetlogger status command was not dispatched") ||
        !Require(handleLookup({ "/combatparser" }) == -2 && dispatch.lastCommand == "/combatparser",
                 "combatparser usage command was not dispatched") ||
        !Require(handleLookup({ "/combatparser", "mode" }) == -2 && dispatch.lastCommand == "/combatparser mode",
                 "combatparser mode command was not dispatched") ||
        !Require(handleLookup({ "/combatparser", "reset" }) == -2 && dispatch.lastCommand == "/combatparser reset",
                 "combatparser reset command was not dispatched") ||
        !Require(handleLookup({ "/combatparser", "lock" }) == -2 && dispatch.lastCommand == "/combatparser lock",
                 "combatparser lock command was not dispatched") ||
        !Require(handleLookup({ "/combatparser", "help" }) == -2 && dispatch.lastCommand == "/combatparser help",
                 "combatparser help command was not dispatched") ||
        !Require(handleLookup({ "/wiki" }) == -2 && dispatch.lastCommand == "/wiki",
                 "wiki home command was not dispatched") ||
        !Require(handleLookup({ "/wiki", "help" }) == -2 && dispatch.lastCommand == "/wiki help",
                 "wiki help command was not dispatched") ||
        !Require(handleLookup({ "/wiki", "aether", "quest" }) == -2 && dispatch.lastCommand == "/wiki aether quest",
                 "wiki query command was not dispatched"))
    {
        return 1;
    }
    if (!Require(handleLookup({ "/wiki", "help", "extra" }) == -2 && dispatch.lastCommand == "/wiki help extra",
                 "wiki multi-word query was not accepted") ||
        !Require(dispatch.calls == 29,
                 "accepted command dispatch count was incorrect"))
    {
        return 1;
    }

    originalState.lookupResult = 42;
    TokenElement nestedToken;
    nestedToken.text = "/pos";
    if (!Require(boundary.HandleLookupForTest(&OriginalLookup, nullptr, &nestedToken, output.data(), false) == 42,
                 "nested lookup did not forward unchanged"))
    {
        return 1;
    }
    boundary.ResetDiagnostics();
    dispatch.throwException    = true;
    originalState.lookupResult = 42;
    std::array<unsigned char, 16> forwardedOutput;
    forwardedOutput.fill(0x5A);
    TokenElement exceptionToken;
    exceptionToken.text                   = "/pos";
    const int forwardedResult             = boundary.HandleLookupForTest(&OriginalLookup,
                                                                         nullptr,
                                                                         &exceptionToken,
                                                                         forwardedOutput.data(),
                                                                         true,
                                                                         &exceptionToken,
                                                                         reinterpret_cast<const unsigned char*>(&exceptionToken) + sizeof(exceptionToken));
    dispatch.throwException               = false;
    const CommandBoundarySnapshot failure = boundary.Snapshot();
    if (!Require(forwardedResult == 42 && failure.dispatchExceptions == 1 && failure.lookupForwarded == 1 && originalState.lookupCalls >= 1,
                 "dispatch exception did not forward to retail") ||
        !Require(forwardedOutput[0] == 0xA5,
                 "forwarded lookup did not write its output"))
    {
        return 1;
    }

    boundary.ResetDiagnostics();
    dispatch.calls            = 0;
    originalState.lookupCalls = 0;
    if (!Require(handleLookup({ "/pos", "window" }) == 42,
                 "incomplete window command did not forward") ||
        !Require(handleLookup({ "/pos", "help", "extra" }) == 42,
                 "help arguments did not forward") ||
        !Require(handleLookup({ "/pos", "window", "ON" }) == 42,
                 "unknown window value did not forward") ||
        !Require(handleLookup({ "/pos", "window", "toggle" }) == 42,
                 "removed window value did not forward") ||
        !Require(handleLookup({ "/pos", "window", "on" }) == 42,
                 "removed window-on command did not forward") ||
        !Require(handleLookup({ "/pos", "window", "on", "extra" }) == 42,
                 "extra command token did not forward") ||
        !Require(handleLookup({ "/positional" }) == 42,
                 "non-pos token did not forward") ||
        !Require(handleLookup({ "/pos", "overlong" }) == 42,
                 "overlong token did not forward") ||
        !Require(handleLookup({ "/fps", "extra" }) == 42,
                 "fps arguments did not forward") ||
        !Require(handleLookup({ "/fps", "help", "extra" }) == 42,
                 "fps help arguments did not forward") ||
        !Require(handleLookup({ "/fps", "lock", "extra" }) == 42,
                 "fps lock arguments did not forward") ||
        !Require(handleLookup({ "/pos", "lock", "extra" }) == 42,
                 "pos lock arguments did not forward") ||
        !Require(handleLookup({ "/distance" }) == 42,
                 "distance without lock did not forward") ||
        !Require(handleLookup({ "/distance", "help", "extra" }) == 42,
                 "distance help arguments did not forward") ||
        !Require(handleLookup({ "/targethp", "lock", "extra" }) == 42,
                 "targethp extra argument did not forward") ||
        !Require(handleLookup({ "/targethp", "help", "extra" }) == 42,
                 "targethp help arguments did not forward") ||
        !Require(handleLookup({ "/packetlogger", "help" }) == 42,
                 "packetlogger unsupported help did not forward") ||
        !Require(handleLookup({ "/packetlogger", "start", "extra" }) == 42,
                 "packetlogger extra argument did not forward") ||
        !Require(handleLookup({ "/combatparser", "save" }) == 42,
                 "combatparser removed save command was still intercepted") ||
        !Require(handleLookup({ "/combatparser", "clear" }) == 42,
                 "combatparser removed clear command was still intercepted") ||
        !Require(handleLookup({ "/combatparser", "self" }) == 42,
                 "combatparser removed self command was still intercepted") ||
        !Require(handleLookup({ "/combatparser", "party" }) == 42,
                 "combatparser removed party command was still intercepted") ||
        !Require(handleLookup({ "/combatparser", "abilities" }) == 42,
                 "combatparser removed abilities command was still intercepted") ||
        !Require(handleLookup({ "/combatparser", "off" }) == 42,
                 "combatparser removed off command was still intercepted") ||
        !Require(handleLookup({ "/combatparser", "status" }) == 42,
                 "combatparser removed status command was still intercepted") ||
        !Require(handleLookup({ "/combatparser", "dps" }) == 42,
                 "combatparser removed dps command was still intercepted") ||
        !Require(handleLookup({ "/combatparser", "hps" }) == 42,
                 "combatparser removed hps command was still intercepted") ||
        !Require(handleLookup({ "/combatparser", "help", "extra" }) == 42,
                 "combatparser help arguments did not forward") ||
        !Require(handleLookup({ "/combatparser", "widget", "party" }) == 42,
                 "combatparser unsupported widget syntax did not forward"))
    {
        return 1;
    }
    TokenElement unreadableToken;
    unreadableToken.text = reinterpret_cast<const char*>(
        static_cast<std::uintptr_t>(1));
    if (!Require(boundary.HandleLookupForTest(&OriginalLookup, nullptr, &unreadableToken, output.data(), true, &unreadableToken, reinterpret_cast<const unsigned char*>(&unreadableToken) + sizeof(unreadableToken)) == 42,
                 "unreadable command pointer did not forward to retail"))
    {
        return 1;
    }
    std::array<TokenElement, 2> unreadableArgumentTokens{};
    unreadableArgumentTokens[0].text = "/pos";
    unreadableArgumentTokens[1].text = reinterpret_cast<const char*>(
        static_cast<std::uintptr_t>(1));
    if (!Require(boundary.HandleLookupForTest(&OriginalLookup, nullptr, unreadableArgumentTokens.data(), output.data(), true, unreadableArgumentTokens.data(), reinterpret_cast<const unsigned char*>(unreadableArgumentTokens.data()) + sizeof(unreadableArgumentTokens)) == 42,
                 "unreadable argument token did not forward to retail"))
    {
        return 1;
    }
    if (!Require(dispatch.calls == 0 && originalState.lookupCalls == 31,
                 "invalid commands were dispatched or not all forwarded"))
    {
        return 1;
    }

    boundary.ResetDiagnostics();
    originalState.lookupResult = 42;
    handleLookup({ "/positional" });
    handleLookup({ "ordinary" });
    const CommandBoundarySnapshot unhandled = boundary.Snapshot();
    if (!Require(unhandled.lookupRecognized == 0 && unhandled.lookupForwarded == 2,
                 "unhandled inputs were not preserved") ||
        !Require(unhandled.lastLookupResult == 42,
                 "unhandled lookup result changed"))
    {
        return 1;
    }

    std::cout << "command-boundary-tests: PASS\n";
    return 0;
}
