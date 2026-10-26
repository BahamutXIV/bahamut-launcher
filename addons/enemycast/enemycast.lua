local locked = false

function load()
    locked = bahamut.settings_get('locked', 'false') == 'true'
end

function command(name)
    if name ~= '/enemycast lock' then
        return false
    end

    locked = not locked
    bahamut.settings_set('locked', locked and 'true' or 'false')
    bahamut.chat_print(locked and 'Enemy Cast overlay locked' or 'Enemy Cast overlay unlocked')
    return true
end

function draw()
    local casts = bahamut.enemy_casts()
    if #casts == 0 then
        return
    end

    local lines = {}
    for index = 1, #casts do
        local cast  = casts[index]
        local actor = cast.actor_name
        if actor == nil or actor == '' then
            actor = string.format('0x%08X', cast.actor_id)
        end

        if not cast.casting then
            lines[#lines + 1] = string.format('%s  Watching for next skill', actor)
        else
            local name        = cast.name:gsub('_', ' ')
            name              = name:gsub('(%a)([%w]*)', function(first, rest)
                return string.upper(first) .. string.lower(rest)
            end)
            local status      = cast.completed and 'Done' or string.format('%.1fs', cast.remaining_seconds)
            lines[#lines + 1] = string.format('%s  %s  %s  %.0f%%',
                actor, name, status, cast.progress * 100)
        end
    end

    bahamut.window('', table.concat(lines, '\n'), locked)
end
