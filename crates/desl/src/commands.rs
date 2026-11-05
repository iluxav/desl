//! The slash command registry: the palette, `/help` and the forwarding rule all read this table.
//! Names come from SPEC.md "Commands, keybindings and CLI".

pub struct SlashCommand {
    pub name: &'static str,
    pub args: &'static str,
    pub desc: &'static str,
    pub built: bool,
}

pub static COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "/switch",
        args: "[agent]",
        desc: "Continue this thread with another agent; alone, pick from a list",
        built: true,
    },
    SlashCommand { name: "/fork", args: "[agent] [@n]", desc: "New thread from here or from message n", built: true },
    SlashCommand { name: "/brief", args: "", desc: "Show the handoff brief desl would send right now", built: true },
    SlashCommand { name: "/new", args: "", desc: "New thread in this project", built: true },
    SlashCommand { name: "/threads", args: "", desc: "Fuzzy thread picker", built: true },
    SlashCommand { name: "/resume", args: "", desc: "Pick a past thread to reopen", built: true },
    SlashCommand { name: "/rename", args: "<name>", desc: "Rename this thread", built: true },
    SlashCommand {
        name: "/model",
        args: "[name] [effort]",
        desc: "Change this agent's model and effort; alone, pick from a list",
        built: true,
    },
    SlashCommand {
        name: "/permissions",
        args: "[mode]",
        desc: "What the agent may do without asking; alone, pick from a list",
        built: true,
    },
    SlashCommand { name: "/clear", args: "", desc: "Start a fresh context on the same thread", built: true },
    SlashCommand { name: "/compact", args: "", desc: "Ask the agent to compact its context", built: false },
    SlashCommand { name: "/export", args: "[file]", desc: "Write the thread as markdown", built: true },
    SlashCommand { name: "/diff", args: "", desc: "Show the worktree diff in a pane", built: false },
    SlashCommand { name: "/pr", args: "", desc: "Create a pull request from this worktree", built: false },
    SlashCommand { name: "/worktree", args: "new|list|rm", desc: "Manage worktrees", built: false },
    SlashCommand { name: "/info", args: "", desc: "Agent, model, native session id, token usage", built: true },
    SlashCommand { name: "/help", args: "", desc: "The help overlay", built: true },
    SlashCommand { name: "/quit", args: "", desc: "Detach and close the client; agents keep running", built: true },
];
