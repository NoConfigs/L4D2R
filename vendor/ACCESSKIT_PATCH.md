vendor/accesskit_windows is accesskit_windows 0.32.1 from crates.io (Apache-2.0 OR MIT),
patched in src/subclass.rs: SubclassImpl::install retries SetPropW and skips the
subclass instead of panicking when Windows refuses it; uninstall is a no-op then.
