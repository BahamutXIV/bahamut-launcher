local RETAIN_PERCENT = 50

local function host_api_available()
    return type(bahamut.action_movement_status) == 'function' and type(bahamut.action_movement_configure) == 'function'
end

local function apply()
    if not host_api_available() then return false end

    return bahamut.action_movement_configure(RETAIN_PERCENT, true)
end

function load()
    apply()
end

function update()
    if not host_api_available() then return end

    local status = bahamut.action_movement_status()
    if status.available and (not status.enabled or status.requested_percent ~= RETAIN_PERCENT) then apply() end
end
