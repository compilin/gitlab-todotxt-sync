Gitlab-todotxt-sync
===================
This is a small CLI utility that will sync your Gitlab TODOs to a local file in the todo.txt format.

Configuration
=============
This file expects a config file in JSON format at `{CONFIG_DIR}/gitlab-todotxt-sync/config.json`, with CONFIG_DIR being the [user configuragion directory](https://docs.rs/dirs/latest/dirs/fn.config_dir.html) (support for CLI argument allowing to point to a different location coming soon). See the definition of the AppConfig struct at the top of main.rs for the options and format.
