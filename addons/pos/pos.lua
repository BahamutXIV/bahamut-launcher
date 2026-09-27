local locked = false

local function position_line()
    local state = bahamut.player_state()
    if state == nil then
        return nil
    end

    local degrees  = math.deg(state.rotation) % 360
    local rotation = math.floor((degrees / 360) * 255 + 0.5)
    return string.format(
        '{ %.3f, %.3f, %.3f, %d }, -- !pos %.3f %.3f %.3f %d',
        state.x, state.z, state.y, rotation,
        state.x, state.y, state.z, state.zone)
end

local function window_line()
    local state = bahamut.player_state()
    if state == nil then
        return nil
    end

    local degrees  = math.deg(state.rotation) % 360
    local rotation = math.floor((degrees / 360) * 255 + 0.5)
    return string.format(
        'X %9.3f | Y %9.3f | Z %9.3f | R %3d | Zone %3d',
        state.x, state.y, state.z, rotation, state.zone)
end

function load()
    locked = bahamut.settings_get('locked', 'false') == 'true'
end

function command(name)
    if name == '/pos' then
        local line = position_line()
        if line ~= nil then
            bahamut.chat_print(line)
            bahamut.clipboard_set(line)
        end

        return true
    end

    if name == '/pos help' then
        bahamut.chat_print('/pos - print and copy your current position')
        bahamut.chat_print('/pos lock - toggle position overlay dragging')
        return true
    end

    if name == '/pos lock' then
        locked = not locked
        bahamut.settings_set('locked', locked and 'true' or 'false')
        bahamut.chat_print(locked and 'Position overlay locked' or 'Position overlay unlocked')
        return true
    end

    return false
end

function draw()
    local line = window_line()
    if line ~= nil then
        bahamut.window('', line, locked)
    end
end
