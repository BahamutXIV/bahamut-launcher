local locked = false

function load()
    locked = bahamut.settings_get('locked', 'false') == 'true'
end

function command(name)
    if name ~= '/customtimer lock' then
        return false
    end

    locked = not locked
    bahamut.settings_set('locked', locked and 'true' or 'false')
    bahamut.chat_print(locked and 'Custom Timer overlay locked' or 'Custom Timer overlay unlocked')
    return true
end

local function time_text(seconds)
    local total     = seconds
    local hours     = math.floor(total / 3600)
    local minutes   = math.floor((total % 3600) / 60)
    local remainder = total % 60
    if hours > 0 then
        return string.format('%d:%02d:%02d', hours, minutes, remainder)
    end

    return string.format('%02d:%02d', minutes, remainder)
end

function draw()
    local timer = bahamut.custom_timer()
    if timer == nil then
        return
    end

    bahamut.window('', string.format('%s | %s | %s',
        timer.instance_name, timer.objective, time_text(timer.remaining_seconds)), locked)
end
