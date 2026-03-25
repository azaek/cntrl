import { useMedia } from "@cntrl-pw/sdk";
import { Button } from "@cntrl-pw/ui/components/button";
import {
    Disc3,
    Pause,
    Play,
    SkipBack,
    SkipForward,
    Volume2,
    VolumeOff,
} from "lucide-react";

const MediaControl = ({ bridgeId }: { bridgeId: string }) => {
    const { data, control } = useMedia(bridgeId);

    return (
        <div className="flex w-full flex-col gap-2 rounded-md border p-2">
            <p className="text-muted-foreground text-xs font-semibold uppercase">Media</p>
            {!data || data.status === "no_media" ? (
                <p className="text-muted-foreground text-xs">No media playing</p>
            ) : (
                <>
                    <div className="flex items-center gap-2">
                        <Disc3 className="text-muted-foreground size-4 animate-spin" />
                        <div className="flex flex-col">
                            <p className="text-sm font-medium">
                                {data.title || "Unknown"}
                            </p>
                            <p className="text-muted-foreground text-xs">
                                {data.artist || "Unknown Artist"}
                            </p>
                        </div>
                    </div>
                    <div className="flex items-center gap-1">
                        <Button
                            size="icon-sm"
                            variant="ghost"
                            onClick={() => control.mutate({ action: "previous" })}
                        >
                            <SkipBack className="size-3.5" />
                        </Button>
                        <Button
                            size="icon-sm"
                            variant="ghost"
                            onClick={() => control.mutate({ action: "play_pause" })}
                        >
                            {data.playing ? (
                                <Pause className="size-3.5" />
                            ) : (
                                <Play className="size-3.5" />
                            )}
                        </Button>
                        <Button
                            size="icon-sm"
                            variant="ghost"
                            onClick={() => control.mutate({ action: "next" })}
                        >
                            <SkipForward className="size-3.5" />
                        </Button>
                        <Button
                            size="icon-sm"
                            variant="ghost"
                            onClick={() => control.mutate({ action: "toggle_mute" })}
                        >
                            {data.muted ? (
                                <VolumeOff className="size-3.5" />
                            ) : (
                                <Volume2 className="size-3.5" />
                            )}
                        </Button>
                        <input
                            type="range"
                            min={0}
                            max={100}
                            value={data.volume ?? 0}
                            onChange={(e) =>
                                control.mutate({
                                    action: "set_volume",
                                    value: Number(e.target.value),
                                })
                            }
                            className="h-1.5 flex-1 cursor-pointer accent-current"
                        />
                        <span className="text-muted-foreground w-8 text-right text-xs tabular-nums">
                            {data.volume ?? 0}%
                        </span>
                    </div>
                </>
            )}
        </div>
    );
};

export default MediaControl;
