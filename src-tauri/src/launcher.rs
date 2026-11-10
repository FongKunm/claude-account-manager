fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(windows)]
    if let Some(result) = claude_account_manager_lib::ordinary::browser_handoff(&args) {
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    #[cfg(feature = "synthetic-tests")]
    {
        if let Err(e) = claude_account_manager_lib::lab::activate() {
            eprintln!("{e}");
            std::process::exit(2);
        }
        if args.first().is_some_and(|s| s == "--cam-lab") {
            match claude_account_manager_lib::lab::control(&args[1..]) {
                Ok(v) => println!("{v}"),
                Err(e) => {
                    println!("{}", serde_json::json!({"error":e}));
                    std::process::exit(1);
                }
            }
            return;
        }
    }
    #[cfg(windows)]
    if args.first().is_some_and(|s| s == "--cam-window-control") {
        let result = claude_account_manager_lib::sessions::control_helper(&args[1..]);
        let failed = result.is_err();
        let value = result.unwrap_or_else(|error| serde_json::json!({"error":error}));
        println!("{value}");
        std::process::exit(if failed { 1 } else { 0 });
    }
    #[cfg(windows)]
    if args.first().is_some_and(|s| s == "--cam-session-helper") {
        let tool = args.last().is_some_and(|s| s == "tool");
        match claude_account_manager_lib::sessions::helper(&args[1..]) {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                if tool {
                    // Errors contain validation reasons, never hook input or credentials.
                    let reason: String = error.chars().filter(|c| !c.is_control()).take(240).collect();
                    eprintln!("换号台未能核对当前工具进程：{reason}");
                    std::process::exit(2)
                } else {
                    std::process::exit(0)
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    if args.first().is_some_and(|s| s == "--import-setup-stdin") {
        let result = if args.len() == 1 {
            claude_account_manager_lib::transfer::import_stdin(true)
        } else if args.len() == 2 && args[1] == "--keep-current" {
            claude_account_manager_lib::transfer::import_stdin(false)
        } else {
            Err("导入只接受标准输入及可选的 --keep-current，不要在命令行里传入令牌。".into())
        };
        if let Err(error) = result {
            eprintln!("Claude 账号管理器：{error}");
            std::process::exit(1);
        }
        return;
    }
    let result = if args.first().is_some_and(|s| s == "--repair-hooks") {
        claude_account_manager_lib::store::Store::discover()
            .and_then(|store| {
                let _lease = store.lock()?;
                claude_account_manager_lib::hooks::install(&store)
            })
            .map(|_| 0)
    } else if args.first().is_some_and(|s| s == "--uninstall-hooks") {
        claude_account_manager_lib::service::Manager::discover()
            .and_then(|manager| {
                manager.uninstall(&claude_account_manager_lib::registry::UserEnvironment)
            })
            .map(|_| 0)
    } else {
        claude_account_manager_lib::client::launch_current(args)
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("Claude 账号管理器：{error}");
            std::process::exit(1);
        }
    }
}
