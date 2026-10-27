local notice_pending = true

-- Printed once while recording, retried until chat_print succeeds.
function update(_)
    if not notice_pending then
        return
    end

    local status = bahamut.packetlog_status()
    if not status.recording then
        return
    end

    local mib = math.floor(status.bytes_on_disk / 1048576 + 0.5)
    if bahamut.chat_print(string.format(
            'Packet capture: on, logs/packets, %d MiB used. Pause with /packetlogger stop. Turn off on the launcher Extensions page',
            mib)) then
        notice_pending = false
    end
end

function command(name)
    if name == '/packetlogger start' then
        if bahamut.packetlog_start() then
            bahamut.chat_print('Packet capture: recording in ' .. bahamut.packetlog_status().file)
        else
            bahamut.chat_print('Packet capture: cannot start recording')
        end

        return true
    end

    if name == '/packetlogger stop' then
        bahamut.chat_print(bahamut.packetlog_stop() and
            'Packet capture: stopped' or 'Packet capture: already stopped')
        return true
    end

    if name == '/packetlogger split' then
        local recording = bahamut.packetlog_status().recording
        if bahamut.packetlog_split() then
            local status = bahamut.packetlog_status()
            bahamut.chat_print(string.format(
                'Packet capture: recording part %d in %s', status.part, status.file))
        elseif recording then
            -- The writer reports the restart or stop that follows a write error.
            bahamut.chat_print('Packet capture: cannot start a new part')
        else
            bahamut.chat_print('Packet capture: not recording')
        end

        return true
    end

    if name == '/packetlogger' or name == '/packetlogger status' then
        local status = bahamut.packetlog_status()
        local file   = status.file
        if status.part > 1 then
            file = string.format('%s (part %d)', file, status.part)
        end

        bahamut.chat_print(string.format(
            'Packet capture: %s | captured: %d, written: %d, dropped: %d, queued: %d, queue peak: %d, skipped: %d | %s',
            status.recording and 'recording' or 'stopped',
            status.captured, status.written, status.dropped,
            status.queued, status.queue_high_water, status.skipped, file))
        return true
    end

    return false
end
