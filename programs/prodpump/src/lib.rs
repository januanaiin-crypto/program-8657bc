use anchor_lang::prelude::*;
use anchor_lang::system_program::{self, Transfer};

declare_id!("GXhVC4QomqL3Md7V578QLZ2P6egbrfXRjvrW9LspymZ9");

const PUMP: Pubkey = pubkey!("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
const PUMP_FEES: Pubkey = pubkey!("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");
const TOKEN: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const TOKEN_2022: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
const WSOL: Pubkey = pubkey!("So11111111111111111111111111111111111111112");
// Same pump layouts as Alliance; offsets include the discriminator.
const SC_DISC: [u8; 8] = [216, 74, 9, 0, 56, 140, 93, 75];
const BC_DISC: [u8; 8] = [23, 183, 248, 55, 96, 216, 172, 96];
const SC_STATUS: usize = 10;
const SC_MINT: usize = 11;
const SC_REVOKED: usize = 75;
const SC_HOLDERS: usize = 76;
const BC_CREATOR: usize = 49;
const BC_MAYHEM: usize = 81;
const BC_QUOTE_MINT: usize = 83;
/// Network fee of the register transaction, refunded to the operator with the record's rent.
const REGISTER_FEE: u64 = 10_000;

#[program]
pub mod prodpump {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>, admin: Pubkey, operator: Pubkey, bind_delay_secs: u32) -> Result<()> {
        check_delay(bind_delay_secs)?;
        let c = &mut ctx.accounts.config;
        c.admin = admin;
        c.operator = operator;
        c.bind_delay_secs = bind_delay_secs;
        c.bump = ctx.bumps.config;
        c.treasury_bump = ctx.bumps.treasury;
        let need = Rent::get()?.minimum_balance(0).saturating_sub(ctx.accounts.treasury.lamports());
        transfer(
            &ctx.accounts.authority.to_account_info(),
            &ctx.accounts.treasury.to_account_info(),
            ctx.accounts.system_program.key(),
            need,
            &[],
        )?;
        emit!(Initialized { admin, operator, bind_delay_secs });
        Ok(())
    }

    pub fn register_product(ctx: Context<RegisterProduct>, product_hash: [u8; 32], launcher: Pubkey) -> Result<()> {
        // Not stopped by the pause: a coin that already launched must still be recorded so the wind-down settles its vault.
        require!(!ctx.accounts.config.closing, ProdPumpError::WindingDown);
        require!(ctx.accounts.vault.lamports() >= Rent::get()?.minimum_balance(0), ProdPumpError::VaultBelowRent);
        read_coin(
            &ctx.accounts.mint,
            &ctx.accounts.sharing_config,
            &ctx.accounts.bonding_curve,
            &ctx.accounts.vault.key(),
            &ctx.accounts.treasury.key(),
        )?;
        let p = &mut ctx.accounts.product;
        p.product_hash = product_hash;
        p.mint = ctx.accounts.mint.key();
        p.launcher = launcher;
        p.registered_at = Clock::get()?.unix_timestamp;
        p.vault_bump = ctx.bumps.vault;
        p.bump = ctx.bumps.product;
        let vault_bump = p.vault_bump;
        // The launch pre-funded the vault with its rent plus this record's rent and fee: hand exactly that back to the
        // operator. Capped, so fees that already reached the vault are never touched.
        let rent = Rent::get()?;
        let cap = rent.minimum_balance(8 + Product::INIT_SPACE).checked_add(REGISTER_FEE).ok_or(ProdPumpError::MathOverflow)?;
        let refund = ctx.accounts.vault.lamports().saturating_sub(rent.minimum_balance(0)).min(cap);
        transfer(
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.operator.to_account_info(),
            ctx.accounts.system_program.key(),
            refund,
            &[&[b"vault", &product_hash, &[vault_bump]]],
        )?;
        let c = &mut ctx.accounts.config;
        c.products_open = c.products_open.checked_add(1).ok_or(ProdPumpError::MathOverflow)?;
        c.products_total = c.products_total.checked_add(1).ok_or(ProdPumpError::MathOverflow)?;
        emit!(ProductRegistered { product_hash, mint: p.mint, launcher, refund });
        Ok(())
    }

    pub fn propose_startup(ctx: Context<OperatorProduct>, product_hash: [u8; 32], wallet: Pubkey) -> Result<()> {
        live(&ctx.accounts.config)?;
        require!(!ctx.accounts.config.closing, ProdPumpError::WindingDown);
        require!(ctx.accounts.product.startup == Pubkey::default(), ProdPumpError::AlreadyBound);
        propose(&mut ctx.accounts.product, product_hash, wallet)
    }

    pub fn cancel_pending(ctx: Context<AdminProduct>, product_hash: [u8; 32]) -> Result<()> {
        ctx.accounts.product.pending = Pubkey::default();
        ctx.accounts.product.pending_since = 0;
        emit!(PendingCancelled { product_hash });
        Ok(())
    }

    pub fn admin_set_startup(ctx: Context<AdminProduct>, product_hash: [u8; 32], wallet: Pubkey) -> Result<()> {
        propose(&mut ctx.accounts.product, product_hash, wallet)
    }

    pub fn activate_startup(ctx: Context<ActivateStartup>, product_hash: [u8; 32]) -> Result<()> {
        live(&ctx.accounts.config)?;
        let p = &mut ctx.accounts.product;
        require!(p.pending != Pubkey::default(), ProdPumpError::NoPending);
        let deadline = p.pending_since.checked_add(i64::from(ctx.accounts.config.bind_delay_secs)).ok_or(ProdPumpError::MathOverflow)?;
        require!(Clock::get()?.unix_timestamp >= deadline, ProdPumpError::TooEarly);
        p.startup = p.pending;
        p.pending = Pubkey::default();
        p.pending_since = 0;
        emit!(StartupActivated { product_hash, wallet: p.startup });
        Ok(())
    }

    pub fn claim(ctx: Context<Claim>, product_hash: [u8; 32]) -> Result<()> {
        pay(
            &ctx.accounts.config,
            &mut ctx.accounts.product,
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.startup.to_account_info(),
            ctx.accounts.system_program.key(),
            product_hash,
        )
    }

    pub fn forward(ctx: Context<Forward>, product_hash: [u8; 32]) -> Result<()> {
        pay(
            &ctx.accounts.config,
            &mut ctx.accounts.product,
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.startup.to_account_info(),
            ctx.accounts.system_program.key(),
            product_hash,
        )
    }

    pub fn withdraw_treasury(ctx: Context<TreasuryAdmin>, lamports: u64) -> Result<()> {
        let keep = if ctx.accounts.config.closing { 0 } else { Rent::get()?.minimum_balance(0) };
        let available = ctx.accounts.treasury.lamports().checked_sub(keep).ok_or(ProdPumpError::InsufficientFunds)?;
        require!(lamports <= available, ProdPumpError::InsufficientFunds);
        transfer(
            &ctx.accounts.treasury.to_account_info(),
            &ctx.accounts.admin.to_account_info(),
            ctx.accounts.system_program.key(),
            lamports,
            &[&[b"treasury", &[ctx.accounts.config.treasury_bump]]],
        )?;
        emit!(TreasuryWithdrawn { amount: lamports, to: ctx.accounts.admin.key() });
        Ok(())
    }

    pub fn set_paused(ctx: Context<AdminOnly>, paused: bool) -> Result<()> {
        require!(paused || !ctx.accounts.config.closing, ProdPumpError::WindingDown);
        ctx.accounts.config.paused = paused;
        emit!(PauseChanged { paused });
        Ok(())
    }

    pub fn set_operator(ctx: Context<AdminOnly>, operator: Pubkey) -> Result<()> {
        ctx.accounts.config.operator = operator;
        emit!(OperatorChanged { operator });
        Ok(())
    }

    pub fn set_bind_delay(ctx: Context<AdminOnly>, bind_delay_secs: u32) -> Result<()> {
        check_delay(bind_delay_secs)?;
        ctx.accounts.config.bind_delay_secs = bind_delay_secs;
        emit!(BindDelayChanged { bind_delay_secs });
        Ok(())
    }

    pub fn propose_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
        ctx.accounts.config.pending_admin = new_admin;
        emit!(AdminProposed { new_admin });
        Ok(())
    }

    pub fn accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
        let c = &mut ctx.accounts.config;
        c.admin = c.pending_admin;
        c.pending_admin = Pubkey::default();
        emit!(AdminAccepted { admin: c.admin });
        Ok(())
    }

    pub fn begin_wind_down(ctx: Context<AdminOnly>) -> Result<()> {
        require!(ctx.accounts.config.paused, ProdPumpError::NotPaused);
        ctx.accounts.config.closing = true;
        emit!(WindDownBegun {});
        Ok(())
    }

    pub fn settle_product(ctx: Context<SettleProduct>, product_hash: [u8; 32]) -> Result<()> {
        require!(ctx.accounts.config.closing, ProdPumpError::NotWindingDown);
        let p = &ctx.accounts.product;
        // unclaimed fees go to the treasury, which close_config then returns to the admin
        let to = if p.startup == Pubkey::default() {
            Pubkey::create_program_address(&[b"treasury", &[ctx.accounts.config.treasury_bump]], &crate::ID).map_err(|_| ProdPumpError::BadVault)?
        } else {
            p.startup
        };
        require_keys_eq!(ctx.accounts.destination.key(), to, ProdPumpError::Unauthorized);
        let amount = ctx.accounts.vault.lamports();
        transfer(
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.destination.to_account_info(),
            ctx.accounts.system_program.key(),
            amount,
            &[&[b"vault", &product_hash, &[p.vault_bump]]],
        )?;
        ctx.accounts.config.products_open = ctx.accounts.config.products_open.checked_sub(1).ok_or(ProdPumpError::MathOverflow)?;
        emit!(Settled { product_hash, amount, to });
        Ok(())
    }

    pub fn close_config(ctx: Context<CloseConfig>) -> Result<()> {
        require!(ctx.accounts.config.closing, ProdPumpError::NotWindingDown);
        require!(ctx.accounts.config.products_open == 0, ProdPumpError::ProductsOpen);
        let amount = ctx.accounts.treasury.lamports();
        transfer(
            &ctx.accounts.treasury.to_account_info(),
            &ctx.accounts.admin.to_account_info(),
            ctx.accounts.system_program.key(),
            amount,
            &[&[b"treasury", &[ctx.accounts.config.treasury_bump]]],
        )?;
        emit!(ConfigClosed { amount, to: ctx.accounts.admin.key() });
        Ok(())
    }
}

fn live(c: &Config) -> Result<()> {
    require!(!c.paused, ProdPumpError::Paused);
    Ok(())
}

fn check_delay(secs: u32) -> Result<()> {
    require!((3600..=30 * 86400).contains(&secs), ProdPumpError::BadParams);
    Ok(())
}

fn propose(p: &mut Product, product_hash: [u8; 32], wallet: Pubkey) -> Result<()> {
    let vault =
        Pubkey::create_program_address(&[b"vault", &product_hash, &[p.vault_bump]], &crate::ID).map_err(|_| ProdPumpError::BadVault)?;
    require!(wallet != Pubkey::default() && wallet != vault, ProdPumpError::BadWallet);
    p.pending = wallet;
    p.pending_since = Clock::get()?.unix_timestamp;
    emit!(StartupProposed { product_hash, wallet, pending_since: p.pending_since });
    Ok(())
}

fn pay<'info>(
    c: &Config,
    p: &mut Product,
    vault: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    system: Pubkey,
    product_hash: [u8; 32],
) -> Result<()> {
    live(c)?;
    require!(p.startup != Pubkey::default(), ProdPumpError::NoStartup);
    let amount = vault.lamports().checked_sub(Rent::get()?.minimum_balance(0)).ok_or(ProdPumpError::NothingToClaim)?;
    require!(amount > 0, ProdPumpError::NothingToClaim);
    p.paid_out = p.paid_out.checked_add(amount).ok_or(ProdPumpError::MathOverflow)?;
    transfer(vault, to, system, amount, &[&[b"vault", &product_hash, &[p.vault_bump]]])?;
    emit!(Claimed { product_hash, amount, to: to.key() });
    Ok(())
}

fn transfer<'info>(from: &AccountInfo<'info>, to: &AccountInfo<'info>, system: Pubkey, amount: u64, seeds: &[&[&[u8]]]) -> Result<()> {
    // An empty vault/treasury must still be closable.
    if amount > 0 {
        system_program::transfer(CpiContext::new_with_signer(system, Transfer { from: from.clone(), to: to.clone() }, seeds), amount)?;
    }
    Ok(())
}

fn rd<const N: usize>(d: &[u8], o: usize) -> Result<[u8; N]> {
    let end = o.checked_add(N).ok_or(ProdPumpError::BadPumpAccount)?;
    d.get(o..end).ok_or(ProdPumpError::BadPumpAccount)?.try_into().map_err(|_| ProdPumpError::BadPumpAccount.into())
}
fn rd_key(d: &[u8], o: usize) -> Result<Pubkey> {
    Ok(Pubkey::new_from_array(rd(d, o)?))
}
fn rd_u8(d: &[u8], o: usize) -> Result<u8> {
    d.get(o).copied().ok_or(ProdPumpError::BadPumpAccount.into())
}

fn read_coin(mint: &AccountInfo, sc: &AccountInfo, bc: &AccountInfo, vault: &Pubkey, treasury: &Pubkey) -> Result<()> {
    require!(*mint.owner == TOKEN || *mint.owner == TOKEN_2022, ProdPumpError::BadPumpAccount);
    // Alliance checks the base mint layout through supply at offset 36.
    let _: [u8; 8] = rd(&mint.try_borrow_data()?, 36)?;
    require_keys_eq!(*sc.owner, PUMP_FEES, ProdPumpError::BadPumpAccount);
    require_keys_eq!(
        sc.key(),
        Pubkey::find_program_address(&[b"sharing-config", mint.key.as_ref()], &PUMP_FEES).0,
        ProdPumpError::BadPumpAccount
    );
    {
        let d = sc.try_borrow_data()?;
        require!(rd::<8>(&d, 0)? == SC_DISC, ProdPumpError::BadPumpAccount);
        require!(rd_u8(&d, SC_STATUS)? == 1 && rd_u8(&d, SC_REVOKED)? == 1, ProdPumpError::BadShares);
        require_keys_eq!(rd_key(&d, SC_MINT)?, mint.key(), ProdPumpError::BadPumpAccount);
        require!(u32::from_le_bytes(rd(&d, SC_HOLDERS)?) == 2, ProdPumpError::BadShares);
        let a = rd_key(&d, 80)?;
        let ab = u16::from_le_bytes(rd(&d, 112)?);
        let b = rd_key(&d, 114)?;
        let bb = u16::from_le_bytes(rd(&d, 146)?);
        require!(
            (a == *vault && ab == 9000 && b == *treasury && bb == 1000) || (b == *vault && bb == 9000 && a == *treasury && ab == 1000),
            ProdPumpError::BadShares
        );
    }
    require_keys_eq!(*bc.owner, PUMP, ProdPumpError::BadPumpAccount);
    require_keys_eq!(
        bc.key(),
        Pubkey::find_program_address(&[b"bonding-curve", mint.key.as_ref()], &PUMP).0,
        ProdPumpError::BadPumpAccount
    );
    let d = bc.try_borrow_data()?;
    require!(rd::<8>(&d, 0)? == BC_DISC, ProdPumpError::BadPumpAccount);
    require_keys_eq!(rd_key(&d, BC_CREATOR)?, sc.key(), ProdPumpError::BadShares);
    require!(rd_u8(&d, BC_MAYHEM)? == 0, ProdPumpError::BadShares);
    let quote = rd_key(&d, BC_QUOTE_MINT)?;
    require!(quote == Pubkey::default() || quote == WSOL, ProdPumpError::BadShares);
    Ok(())
}

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub admin: Pubkey,
    pub pending_admin: Pubkey,
    pub operator: Pubkey,
    pub paused: bool,
    pub closing: bool,
    pub bind_delay_secs: u32,
    pub products_open: u64,
    pub products_total: u64,
    pub bump: u8,
    pub treasury_bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Product {
    pub product_hash: [u8; 32],
    pub mint: Pubkey,
    pub launcher: Pubkey,
    pub registered_at: i64,
    pub startup: Pubkey,
    pub pending: Pubkey,
    pub pending_since: i64,
    pub paid_out: u64,
    pub vault_bump: u8,
    pub bump: u8,
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(init, payer = authority, space = 8 + Config::INIT_SPACE, seeds = [b"config"], bump)]
    pub config: Account<'info, Config>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()) @ ProdPumpError::Unauthorized)]
    pub program: Program<'info, crate::program::Prodpump>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key()) @ ProdPumpError::Unauthorized)]
    pub program_data: Account<'info, ProgramData>,
    #[account(mut, seeds = [b"treasury"], bump, constraint = treasury.data_is_empty() @ ProdPumpError::BadVault)]
    pub treasury: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(product_hash: [u8; 32])]
pub struct RegisterProduct<'info> {
    #[account(mut)]
    pub operator: Signer<'info>,
    #[account(mut, seeds = [b"config"], bump = config.bump, has_one = operator @ ProdPumpError::Unauthorized)]
    pub config: Account<'info, Config>,
    #[account(init, payer = operator, space = 8 + Product::INIT_SPACE, seeds = [b"product".as_ref(), product_hash.as_ref()], bump)]
    pub product: Account<'info, Product>,
    #[account(mut, seeds = [b"vault".as_ref(), product_hash.as_ref()], bump, constraint = vault.data_is_empty() @ ProdPumpError::BadVault)]
    pub vault: SystemAccount<'info>,
    #[account(seeds = [b"treasury"], bump = config.treasury_bump, constraint = treasury.data_is_empty() @ ProdPumpError::BadVault)]
    pub treasury: SystemAccount<'info>,
    /// CHECK: token owner and base mint layout checked in read_coin.
    pub mint: UncheckedAccount<'info>,
    /// CHECK: pump_fees owner, PDA, discriminator and locked shares checked in read_coin.
    pub sharing_config: UncheckedAccount<'info>,
    /// CHECK: pump owner, PDA, discriminator and fee routing checked in read_coin.
    pub bonding_curve: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(product_hash: [u8; 32])]
pub struct OperatorProduct<'info> {
    pub operator: Signer<'info>,
    #[account(seeds = [b"config"], bump = config.bump, has_one = operator @ ProdPumpError::Unauthorized)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [b"product".as_ref(), product_hash.as_ref()], bump = product.bump)]
    pub product: Account<'info, Product>,
}

#[derive(Accounts)]
#[instruction(product_hash: [u8; 32])]
pub struct AdminProduct<'info> {
    pub admin: Signer<'info>,
    #[account(seeds = [b"config"], bump = config.bump, has_one = admin @ ProdPumpError::Unauthorized)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [b"product".as_ref(), product_hash.as_ref()], bump = product.bump)]
    pub product: Account<'info, Product>,
}

#[derive(Accounts)]
#[instruction(product_hash: [u8; 32])]
pub struct ActivateStartup<'info> {
    #[account(seeds = [b"config"], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [b"product".as_ref(), product_hash.as_ref()], bump = product.bump)]
    pub product: Account<'info, Product>,
}

#[derive(Accounts)]
#[instruction(product_hash: [u8; 32])]
pub struct Claim<'info> {
    #[account(mut, address = product.startup @ ProdPumpError::Unauthorized)]
    pub startup: Signer<'info>,
    #[account(seeds = [b"config"], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [b"product".as_ref(), product_hash.as_ref()], bump = product.bump)]
    pub product: Account<'info, Product>,
    #[account(mut, seeds = [b"vault".as_ref(), product_hash.as_ref()], bump = product.vault_bump, constraint = vault.data_is_empty() @ ProdPumpError::BadVault)]
    pub vault: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(product_hash: [u8; 32])]
pub struct Forward<'info> {
    /// CHECK: only the bound startup can receive SOL.
    #[account(mut, address = product.startup @ ProdPumpError::Unauthorized)]
    pub startup: UncheckedAccount<'info>,
    #[account(seeds = [b"config"], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [b"product".as_ref(), product_hash.as_ref()], bump = product.bump)]
    pub product: Account<'info, Product>,
    #[account(mut, seeds = [b"vault".as_ref(), product_hash.as_ref()], bump = product.vault_bump, constraint = vault.data_is_empty() @ ProdPumpError::BadVault)]
    pub vault: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [b"config"], bump = config.bump, has_one = admin @ ProdPumpError::Unauthorized)]
    pub config: Account<'info, Config>,
}

#[derive(Accounts)]
pub struct AcceptAdmin<'info> {
    pub pending_admin: Signer<'info>,
    #[account(mut, seeds = [b"config"], bump = config.bump, has_one = pending_admin @ ProdPumpError::Unauthorized)]
    pub config: Account<'info, Config>,
}

#[derive(Accounts)]
pub struct TreasuryAdmin<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(seeds = [b"config"], bump = config.bump, has_one = admin @ ProdPumpError::Unauthorized)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [b"treasury"], bump = config.treasury_bump, constraint = treasury.data_is_empty() @ ProdPumpError::BadVault)]
    pub treasury: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(product_hash: [u8; 32])]
pub struct SettleProduct<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(mut, seeds = [b"config"], bump = config.bump, has_one = admin @ ProdPumpError::Unauthorized)]
    pub config: Account<'info, Config>,
    #[account(mut, close = admin, seeds = [b"product".as_ref(), product_hash.as_ref()], bump = product.bump)]
    pub product: Account<'info, Product>,
    #[account(mut, seeds = [b"vault".as_ref(), product_hash.as_ref()], bump = product.vault_bump, constraint = vault.data_is_empty() @ ProdPumpError::BadVault)]
    pub vault: SystemAccount<'info>,
    /// CHECK: bound startup, or the treasury if unbound; checked in handler.
    #[account(mut)]
    pub destination: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CloseConfig<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(mut, close = admin, seeds = [b"config"], bump = config.bump, has_one = admin @ ProdPumpError::Unauthorized)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [b"treasury"], bump = config.treasury_bump, constraint = treasury.data_is_empty() @ ProdPumpError::BadVault)]
    pub treasury: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[event]
pub struct Initialized {
    pub admin: Pubkey,
    pub operator: Pubkey,
    pub bind_delay_secs: u32,
}
#[event]
pub struct ProductRegistered {
    pub product_hash: [u8; 32],
    pub mint: Pubkey,
    pub launcher: Pubkey,
    pub refund: u64,
}
#[event]
pub struct StartupProposed {
    pub product_hash: [u8; 32],
    pub wallet: Pubkey,
    pub pending_since: i64,
}
#[event]
pub struct PendingCancelled {
    pub product_hash: [u8; 32],
}
#[event]
pub struct StartupActivated {
    pub product_hash: [u8; 32],
    pub wallet: Pubkey,
}
#[event]
pub struct Claimed {
    pub product_hash: [u8; 32],
    pub amount: u64,
    pub to: Pubkey,
}
#[event]
pub struct TreasuryWithdrawn {
    pub amount: u64,
    pub to: Pubkey,
}
#[event]
pub struct PauseChanged {
    pub paused: bool,
}
#[event]
pub struct OperatorChanged {
    pub operator: Pubkey,
}
#[event]
pub struct BindDelayChanged {
    pub bind_delay_secs: u32,
}
#[event]
pub struct AdminProposed {
    pub new_admin: Pubkey,
}
#[event]
pub struct AdminAccepted {
    pub admin: Pubkey,
}
#[event]
pub struct WindDownBegun {}
#[event]
pub struct Settled {
    pub product_hash: [u8; 32],
    pub amount: u64,
    pub to: Pubkey,
}
#[event]
pub struct ConfigClosed {
    pub amount: u64,
    pub to: Pubkey,
}

#[error_code]
pub enum ProdPumpError {
    #[msg("Unauthorized")]
    Unauthorized,
    #[msg("Paused")]
    Paused,
    #[msg("Binding delay must be between one hour and 30 days")]
    BadParams,
    #[msg("Invalid pump account")]
    BadPumpAccount,
    #[msg("Coin must lock exactly vault 9000 / treasury 1000 and use plain SOL")]
    BadShares,
    #[msg("Arithmetic overflow")]
    MathOverflow,
    #[msg("Wind-down requires pause")]
    NotPaused,
    #[msg("Winding down")]
    WindingDown,
    #[msg("Not winding down")]
    NotWindingDown,
    #[msg("Products still open")]
    ProductsOpen,
    #[msg("Only admin can change an existing binding")]
    AlreadyBound,
    #[msg("No pending binding")]
    NoPending,
    #[msg("Binding delay has not elapsed")]
    TooEarly,
    #[msg("No startup bound")]
    NoStartup,
    #[msg("Nothing above vault rent to claim")]
    NothingToClaim,
    #[msg("Insufficient treasury balance")]
    InsufficientFunds,
    #[msg("Vault must be data-less")]
    BadVault,
    #[msg("Vault must be rent-exempt before registration")]
    VaultBelowRent,
    #[msg("Startup wallet cannot be the none sentinel or its own vault")]
    BadWallet,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pump_reads_are_bounded_and_delay_is_inclusive() {
        assert!(rd_key(&[0; 31], 0).is_err());
        assert!(rd::<8>(&[0; 8], usize::MAX).is_err());
        assert!(rd_u8(&[], 0).is_err());
        assert!(check_delay(3599).is_err());
        assert!(check_delay(3600).is_ok());
        assert!(check_delay(30 * 86400).is_ok());
        assert!(check_delay(30 * 86400 + 1).is_err());
    }
}
