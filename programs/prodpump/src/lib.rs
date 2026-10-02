#![no_std]

use pinocchio::cpi::{Seed, Signer};
use pinocchio::error::ProgramError;
use pinocchio::sysvars::{clock::Clock, rent::Rent, Sysvar};
use pinocchio::{AccountView, Address, ProgramResult};
use pinocchio_system::instructions::{Allocate, Assign, Transfer};

#[cfg(not(feature = "no-entrypoint"))]
pinocchio::program_entrypoint!(process_instruction, 9);
pinocchio::no_allocator!();
pinocchio::nostd_panic_handler!();

pub const ID: Address = Address::from_str_const("GXhVC4QomqL3Md7V578QLZ2P6egbrfXRjvrW9LspymZ9");
const SYSTEM: Address = pinocchio_system::ID;
const LOADER: Address = Address::from_str_const("BPFLoaderUpgradeab1e11111111111111111111111");
const PUMP: Address = Address::from_str_const("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
const PUMP_FEES: Address = Address::from_str_const("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");
const TOKEN: Address = Address::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const TOKEN_2022: Address = Address::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
const WSOL: Address = Address::from_str_const("So11111111111111111111111111111111111111112");
const ZERO: Address = Address::new_from_array([0; 32]);
const CONFIG_LEN: usize = 128;
const PRODUCT_LEN: usize = 194;
const CONFIG_DISC: [u8; 8] = [155, 12, 170, 224, 30, 250, 204, 130];
const PRODUCT_DISC: [u8; 8] = [102, 76, 55, 251, 38, 73, 224, 229];
const SC_DISC: [u8; 8] = [216, 74, 9, 0, 56, 140, 93, 75];
const BC_DISC: [u8; 8] = [23, 183, 248, 55, 96, 216, 172, 96];
const REGISTER_FEE: u64 = 10_000;

// Anchor borsh offsets, including the 8-byte discriminator (no Rust struct padding).
mod c {
    pub const ADMIN: usize = 8;
    pub const PENDING: usize = 40;
    pub const OPERATOR: usize = 72;
    pub const PAUSED: usize = 104;
    pub const CLOSING: usize = 105;
    pub const DELAY: usize = 106;
    pub const OPEN: usize = 110;
    pub const TOTAL: usize = 118;
    pub const BUMP: usize = 126;
    pub const TREASURY_BUMP: usize = 127;
}
mod p {
    pub const HASH: usize = 8;
    pub const MINT: usize = 40;
    pub const LAUNCHER: usize = 72;
    pub const REGISTERED: usize = 104;
    pub const STARTUP: usize = 112;
    pub const PENDING: usize = 144;
    pub const SINCE: usize = 176;
    pub const PAID: usize = 184;
    pub const VAULT_BUMP: usize = 192;
    pub const BUMP: usize = 193;
}
// Preserve ProdPumpError's order: Unauthorized=6000 ... BadWallet=6018.
#[repr(u32)]
enum Error {
    Unauthorized = 6000,
    Paused,
    BadParams,
    BadPumpAccount,
    BadShares,
    MathOverflow,
    NotPaused,
    WindingDown,
    NotWindingDown,
    ProductsOpen,
    AlreadyBound,
    NoPending,
    TooEarly,
    NoStartup,
    NothingToClaim,
    InsufficientFunds,
    BadVault,
    VaultBelowRent,
    BadWallet,
}
fn err(e: Error) -> ProgramError {
    ProgramError::Custom(e as u32)
}
fn need(ok: bool, e: Error) -> ProgramResult {
    if ok {
        Ok(())
    } else {
        Err(err(e))
    }
}
fn rd<const N: usize>(d: &[u8], o: usize) -> Result<[u8; N], ProgramError> {
    let end = o.checked_add(N).ok_or(ProgramError::InvalidAccountData)?;
    d.get(o..end)
        .ok_or(ProgramError::InvalidAccountData)?
        .try_into()
        .map_err(|_| ProgramError::InvalidAccountData)
}
fn byte(d: &[u8], o: usize) -> Result<u8, ProgramError> {
    d.get(o).copied().ok_or(ProgramError::InvalidAccountData)
}
fn key(d: &[u8], o: usize) -> Result<Address, ProgramError> {
    Ok(Address::new_from_array(rd(d, o)?))
}
fn u64_at(d: &[u8], o: usize) -> Result<u64, ProgramError> {
    Ok(u64::from_le_bytes(rd(d, o)?))
}
fn put(d: &mut [u8], o: usize, v: &[u8]) -> ProgramResult {
    let end = o
        .checked_add(v.len())
        .ok_or(ProgramError::InvalidAccountData)?;
    d.get_mut(o..end)
        .ok_or(ProgramError::InvalidAccountData)?
        .copy_from_slice(v);
    Ok(())
}
fn add(a: u64, b: u64) -> Result<u64, ProgramError> {
    a.checked_add(b).ok_or(err(Error::MathOverflow))
}
fn sub(a: u64, b: u64) -> Result<u64, ProgramError> {
    a.checked_sub(b).ok_or(err(Error::MathOverflow))
}
fn account(a: &[AccountView], i: usize) -> Result<&AccountView, ProgramError> {
    a.get(i).ok_or(ProgramError::NotEnoughAccountKeys)
}
fn writable(a: &AccountView) -> ProgramResult {
    need(a.is_writable(), Error::Unauthorized)
}
fn signer(a: &AccountView) -> ProgramResult {
    if a.is_signer() {
        Ok(())
    } else {
        Err(ProgramError::MissingRequiredSignature)
    }
}
fn system(a: &AccountView) -> ProgramResult {
    need(a.address() == &SYSTEM && a.executable(), Error::BadVault)
}
fn find(seeds: &[&[u8]], a: &AccountView) -> Result<u8, ProgramError> {
    let (address, bump) = Address::find_program_address(seeds, &ID);
    need(a.address() == &address, Error::BadVault)?;
    Ok(bump)
}
fn derived(seeds: &[&[u8]]) -> Result<Address, ProgramError> {
    Address::create_program_address(seeds, &ID).map_err(|_| err(Error::BadVault))
}
fn check_pda(a: &AccountView, seeds: &[&[u8]]) -> ProgramResult {
    need(a.address() == &derived(seeds)?, Error::BadVault)
}
fn money(a: &AccountView, seeds: &[&[u8]]) -> ProgramResult {
    check_pda(a, seeds)?;
    need(a.owned_by(&SYSTEM) && a.data_len() == 0, Error::BadVault)
}
fn load<const N: usize>(a: &AccountView, disc: &[u8; 8]) -> Result<[u8; N], ProgramError> {
    if !a.owned_by(&ID) {
        return Err(ProgramError::IllegalOwner);
    }
    if a.data_len() != N {
        return Err(ProgramError::InvalidAccountData);
    }
    let data = a.try_borrow()?;
    if rd::<8>(&data, 0)? != *disc {
        return Err(ProgramError::InvalidAccountData);
    }
    rd(&data, 0)
}
fn config(a: &AccountView) -> Result<[u8; CONFIG_LEN], ProgramError> {
    let d = load(a, &CONFIG_DISC)?;
    check_pda(a, &[b"config", &[byte(&d, c::BUMP)?]])?;
    need(
        byte(&d, c::PAUSED)? <= 1 && byte(&d, c::CLOSING)? <= 1,
        Error::BadParams,
    )?;
    Ok(d)
}
fn product(a: &AccountView, hash: &[u8; 32]) -> Result<[u8; PRODUCT_LEN], ProgramError> {
    let d = load(a, &PRODUCT_DISC)?;
    check_pda(a, &[b"product", hash, &[byte(&d, p::BUMP)?]])?;
    need(rd::<32>(&d, p::HASH)? == *hash, Error::BadParams)?;
    Ok(d)
}
fn save(a: &mut AccountView, d: &[u8]) -> ProgramResult {
    writable(a)?;
    let mut data = a.try_borrow_mut()?;
    if data.len() != d.len() {
        return Err(ProgramError::InvalidAccountData);
    }
    data.copy_from_slice(d);
    Ok(())
}
fn auth(d: &[u8], offset: usize, a: &AccountView) -> ProgramResult {
    signer(a)?;
    need(a.address() == &key(d, offset)?, Error::Unauthorized)
}
fn live(d: &[u8]) -> ProgramResult {
    need(byte(d, c::PAUSED)? == 0, Error::Paused)
}
fn delay(n: u32) -> ProgramResult {
    need((3600..=30 * 86400).contains(&n), Error::BadParams)
}
fn now() -> Result<i64, ProgramError> {
    Ok(Clock::get()?.unix_timestamp)
}
fn rent(len: usize) -> Result<u64, ProgramError> {
    Rent::get()?.try_minimum_balance(len)
}
fn transfer(from: &AccountView, to: &AccountView, lamports: u64, seeds: &[Seed]) -> ProgramResult {
    writable(from)?;
    writable(to)?;
    if lamports == 0 {
        return Ok(());
    }
    let ix = Transfer { from, to, lamports };
    if seeds.is_empty() {
        ix.invoke()
    } else {
        ix.invoke_signed(&[Signer::from(seeds)])
    }
}
fn init_account(
    payer: &AccountView,
    new: &AccountView,
    len: usize,
    seeds: &[Seed],
) -> ProgramResult {
    signer(payer)?;
    writable(payer)?;
    writable(new)?;
    // Never overwrite existing records; prefunding an empty system PDA remains harmless.
    if !new.owned_by(&SYSTEM) || new.data_len() != 0 {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    let shortfall = rent(len)?.saturating_sub(new.lamports());
    transfer(payer, new, shortfall, &[])?;
    let signers = [Signer::from(seeds)];
    Allocate {
        account: new,
        space: len as u64,
    }
    .invoke_signed(&signers)?;
    Assign {
        account: new,
        owner: &ID,
    }
    .invoke_signed(&signers)
}
fn close_account(a: &mut [AccountView], from: usize, to: usize) -> ProgramResult {
    let src = account(a, from)?;
    let dst = account(a, to)?;
    writable(src)?;
    writable(dst)?;
    need(src.address() != dst.address(), Error::BadWallet)?;
    let balance = add(dst.lamports(), src.lamports())?;
    let src = a.get_mut(from).ok_or(ProgramError::NotEnoughAccountKeys)?;
    src.try_borrow_mut()?.fill(0);
    // close() sets length=0, lamports=0 and owner=system before any later instruction/CPI.
    src.close()?;
    a.get_mut(to)
        .ok_or(ProgramError::NotEnoughAccountKeys)?
        .set_lamports(balance);
    Ok(())
}
fn emit(disc: [u8; 8], fields: &[&[u8]]) -> ProgramResult {
    let mut d = [0u8; 112];
    put(&mut d, 0, &disc)?;
    let mut len = 8usize;
    for field in fields {
        put(&mut d, len, field)?;
        len = len
            .checked_add(field.len())
            .ok_or(err(Error::MathOverflow))?;
    }
    #[cfg(target_os = "solana")]
    {
        let raw = [(d.as_ptr(), len as u64)];
        // SAFETY: the slice is initialized and lives through this syscall.
        unsafe { pinocchio::syscalls::sol_log_data(raw.as_ptr() as *const u8, 1) };
    }
    Ok(())
}

pub fn process_instruction(pid: &Address, a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    need(pid == &ID, Error::Unauthorized)?;
    let disc = rd::<8>(d, 0).map_err(|_| ProgramError::InvalidInstructionData)?;
    let args = d.get(8..).ok_or(ProgramError::InvalidInstructionData)?;
    match disc {
        [175, 175, 109, 31, 13, 152, 155, 237] => initialize(a, args),
        [224, 97, 195, 220, 124, 218, 78, 43] => register(a, args),
        [12, 122, 26, 208, 29, 5, 23, 192] => binding(a, args, 0),
        [74, 87, 109, 242, 64, 192, 151, 71] => binding(a, args, 1),
        [242, 22, 205, 239, 181, 178, 2, 142] => binding(a, args, 2),
        [138, 51, 177, 123, 68, 20, 217, 232] => activate(a, args),
        [62, 198, 214, 193, 213, 159, 108, 210] => pay(a, args, true),
        [45, 165, 201, 116, 206, 225, 241, 18] => pay(a, args, false),
        [40, 63, 122, 158, 144, 216, 83, 96] => withdraw(a, args),
        [91, 60, 125, 192, 176, 225, 166, 218] => admin(a, args, 0),
        [238, 153, 101, 169, 243, 131, 36, 1] => admin(a, args, 1),
        [60, 216, 31, 237, 127, 166, 230, 160] => admin(a, args, 2),
        [121, 214, 199, 212, 87, 39, 117, 234] => admin(a, args, 3),
        [112, 42, 45, 90, 116, 181, 13, 170] => admin(a, args, 4),
        [89, 28, 134, 161, 28, 9, 243, 171] => admin(a, args, 5),
        [129, 188, 250, 212, 170, 252, 188, 65] => settle(a, args),
        [145, 9, 72, 157, 95, 125, 61, 85] => close_config(a),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn initialize(a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    let authority = account(a, 0)?;
    signer(authority)?;
    writable(authority)?;
    writable(account(a, 1)?)?;
    writable(account(a, 4)?)?;
    system(account(a, 5)?)?;
    let program = account(a, 2)?;
    let pd = account(a, 3)?;
    need(
        program.address() == &ID && program.owned_by(&LOADER) && program.executable(),
        Error::Unauthorized,
    )?;
    let pd_key = Address::find_program_address(&[ID.as_ref()], &LOADER).0;
    need(
        pd.address() == &pd_key && pd.owned_by(&LOADER),
        Error::Unauthorized,
    )?;
    {
        let data = program.try_borrow()?;
        need(
            rd::<4>(&data, 0)? == 2u32.to_le_bytes() && key(&data, 4)? == pd_key,
            Error::Unauthorized,
        )?;
        let data = pd.try_borrow()?;
        need(
            rd::<4>(&data, 0)? == 3u32.to_le_bytes()
                && byte(&data, 12)? == 1
                && key(&data, 13)? == *authority.address(),
            Error::Unauthorized,
        )?;
    }
    let admin = key(d, 0)?;
    let operator = key(d, 32)?;
    let secs = u32::from_le_bytes(rd(d, 64)?);
    delay(secs)?;
    let bump = [find(&[b"config"], account(a, 1)?)?];
    let tb = [find(&[b"treasury"], account(a, 4)?)?];
    money(account(a, 4)?, &[b"treasury", &tb])?;
    init_account(
        authority,
        account(a, 1)?,
        CONFIG_LEN,
        &[Seed::from(b"config"), Seed::from(&bump)],
    )?;
    transfer(
        authority,
        account(a, 4)?,
        rent(0)?.saturating_sub(account(a, 4)?.lamports()),
        &[],
    )?;
    let mut c = [0u8; CONFIG_LEN];
    put(&mut c, 0, &CONFIG_DISC)?;
    put(&mut c, c::ADMIN, admin.as_ref())?;
    put(&mut c, c::OPERATOR, operator.as_ref())?;
    put(&mut c, c::DELAY, &secs.to_le_bytes())?;
    put(&mut c, c::BUMP, &bump)?;
    put(&mut c, c::TREASURY_BUMP, &tb)?;
    save(a.get_mut(1).ok_or(ProgramError::NotEnoughAccountKeys)?, &c)?;
    emit(
        [208, 213, 115, 98, 115, 82, 201, 209],
        &[admin.as_ref(), operator.as_ref(), &secs.to_le_bytes()],
    )
}

fn register(a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    let hash = rd::<32>(d, 0)?;
    let launcher = key(d, 32)?;
    let mut c = config(account(a, 1)?)?;
    auth(&c, c::OPERATOR, account(a, 0)?)?;
    for i in 0..4 {
        writable(account(a, i)?)?;
    }
    system(account(a, 8)?)?;
    need(byte(&c, c::CLOSING)? == 0, Error::WindingDown)?;
    let bump = [find(&[b"product", &hash], account(a, 2)?)?];
    let vb = [find(&[b"vault", &hash], account(a, 3)?)?];
    money(account(a, 3)?, &[b"vault", &hash, &vb])?;
    money(
        account(a, 4)?,
        &[b"treasury", &[byte(&c, c::TREASURY_BUMP)?]],
    )?;
    need(account(a, 3)?.lamports() >= rent(0)?, Error::VaultBelowRent)?;
    read_coin(
        account(a, 5)?,
        account(a, 6)?,
        account(a, 7)?,
        account(a, 3)?.address(),
        account(a, 4)?.address(),
    )?;
    init_account(
        account(a, 0)?,
        account(a, 2)?,
        PRODUCT_LEN,
        &[Seed::from(b"product"), Seed::from(&hash), Seed::from(&bump)],
    )?;
    let mint = *account(a, 5)?.address();
    let mut p = [0u8; PRODUCT_LEN];
    put(&mut p, 0, &PRODUCT_DISC)?;
    put(&mut p, p::HASH, &hash)?;
    put(&mut p, p::MINT, mint.as_ref())?;
    put(&mut p, p::LAUNCHER, launcher.as_ref())?;
    put(&mut p, p::REGISTERED, &now()?.to_le_bytes())?;
    put(&mut p, p::VAULT_BUMP, &vb)?;
    put(&mut p, p::BUMP, &bump)?;
    let cap = add(rent(PRODUCT_LEN)?, REGISTER_FEE)?;
    let refund = account(a, 3)?.lamports().saturating_sub(rent(0)?).min(cap);
    transfer(
        account(a, 3)?,
        account(a, 0)?,
        refund,
        &[Seed::from(b"vault"), Seed::from(&hash), Seed::from(&vb)],
    )?;
    let open = add(u64_at(&c, c::OPEN)?, 1)?;
    let total = add(u64_at(&c, c::TOTAL)?, 1)?;
    put(&mut c, c::OPEN, &open.to_le_bytes())?;
    put(&mut c, c::TOTAL, &total.to_le_bytes())?;
    save(a.get_mut(1).ok_or(ProgramError::NotEnoughAccountKeys)?, &c)?;
    save(a.get_mut(2).ok_or(ProgramError::NotEnoughAccountKeys)?, &p)?;
    emit(
        [116, 97, 68, 213, 201, 211, 170, 221],
        &[
            &hash,
            mint.as_ref(),
            launcher.as_ref(),
            &refund.to_le_bytes(),
        ],
    )
}

fn binding(a: &mut [AccountView], d: &[u8], mode: u8) -> ProgramResult {
    let hash = rd::<32>(d, 0)?;
    let c = config(account(a, 1)?)?;
    auth(
        &c,
        if mode == 0 { c::OPERATOR } else { c::ADMIN },
        account(a, 0)?,
    )?;
    let mut p = product(account(a, 2)?, &hash)?;
    writable(account(a, 2)?)?;
    if mode == 0 {
        live(&c)?;
        need(byte(&c, c::CLOSING)? == 0, Error::WindingDown)?;
        need(key(&p, p::STARTUP)? == ZERO, Error::AlreadyBound)?;
    }
    let (wallet, since) = if mode == 1 {
        (ZERO, 0)
    } else {
        let wallet = key(d, 32)?;
        let vault = derived(&[b"vault", &hash, &[byte(&p, p::VAULT_BUMP)?]])?;
        let treasury = derived(&[b"treasury", &[byte(&c, c::TREASURY_BUMP)?]])?;
        need(
            wallet != ZERO
                && wallet != vault
                && wallet != treasury
                && wallet != *account(a, 1)?.address()
                && wallet != *account(a, 2)?.address(),
            Error::BadWallet,
        )?;
        (wallet, now()?)
    };
    put(&mut p, p::PENDING, wallet.as_ref())?;
    put(&mut p, p::SINCE, &since.to_le_bytes())?;
    save(a.get_mut(2).ok_or(ProgramError::NotEnoughAccountKeys)?, &p)?;
    if mode == 1 {
        emit([24, 20, 230, 224, 239, 65, 117, 159], &[&hash])
    } else {
        emit(
            [138, 219, 173, 132, 221, 231, 13, 143],
            &[&hash, wallet.as_ref(), &since.to_le_bytes()],
        )
    }
}

fn activate(a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    let hash = rd::<32>(d, 0)?;
    let c = config(account(a, 0)?)?;
    let mut p = product(account(a, 1)?, &hash)?;
    writable(account(a, 1)?)?;
    live(&c)?;
    let wallet = key(&p, p::PENDING)?;
    need(wallet != ZERO, Error::NoPending)?;
    let since = i64::from_le_bytes(rd(&p, p::SINCE)?);
    let deadline = since
        .checked_add(i64::from(u32::from_le_bytes(rd(&c, c::DELAY)?)))
        .ok_or(err(Error::MathOverflow))?;
    need(now()? >= deadline, Error::TooEarly)?;
    put(&mut p, p::STARTUP, wallet.as_ref())?;
    put(&mut p, p::PENDING, ZERO.as_ref())?;
    put(&mut p, p::SINCE, &0i64.to_le_bytes())?;
    save(a.get_mut(1).ok_or(ProgramError::NotEnoughAccountKeys)?, &p)?;
    emit(
        [78, 47, 84, 123, 55, 230, 161, 15],
        &[&hash, wallet.as_ref()],
    )
}

fn pay(a: &mut [AccountView], d: &[u8], claim: bool) -> ProgramResult {
    let hash = rd::<32>(d, 0)?;
    let c = config(account(a, 1)?)?;
    let mut p = product(account(a, 2)?, &hash)?;
    if claim {
        signer(account(a, 0)?)?;
    }
    for i in [0, 2, 3] {
        writable(account(a, i)?)?;
    }
    system(account(a, 4)?)?;
    let wallet = key(&p, p::STARTUP)?;
    need(account(a, 0)?.address() == &wallet, Error::Unauthorized)?;
    let vb = [byte(&p, p::VAULT_BUMP)?];
    money(account(a, 3)?, &[b"vault", &hash, &vb])?;
    live(&c)?;
    need(wallet != ZERO, Error::NoStartup)?;
    let amount = account(a, 3)?
        .lamports()
        .checked_sub(rent(0)?)
        .ok_or(err(Error::NothingToClaim))?;
    need(amount > 0, Error::NothingToClaim)?;
    let paid = add(u64_at(&p, p::PAID)?, amount)?;
    put(&mut p, p::PAID, &paid.to_le_bytes())?;
    transfer(
        account(a, 3)?,
        account(a, 0)?,
        amount,
        &[Seed::from(b"vault"), Seed::from(&hash), Seed::from(&vb)],
    )?;
    save(a.get_mut(2).ok_or(ProgramError::NotEnoughAccountKeys)?, &p)?;
    emit(
        [217, 192, 123, 72, 108, 150, 248, 33],
        &[&hash, &amount.to_le_bytes(), wallet.as_ref()],
    )
}

fn withdraw(a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    let c = config(account(a, 1)?)?;
    auth(&c, c::ADMIN, account(a, 0)?)?;
    writable(account(a, 0)?)?;
    writable(account(a, 2)?)?;
    system(account(a, 3)?)?;
    let tb = [byte(&c, c::TREASURY_BUMP)?];
    money(account(a, 2)?, &[b"treasury", &tb])?;
    let amount = u64_at(d, 0)?;
    let keep = if byte(&c, c::CLOSING)? == 1 {
        0
    } else {
        rent(0)?
    };
    let available = account(a, 2)?
        .lamports()
        .checked_sub(keep)
        .ok_or(err(Error::InsufficientFunds))?;
    need(amount <= available, Error::InsufficientFunds)?;
    transfer(
        account(a, 2)?,
        account(a, 0)?,
        amount,
        &[Seed::from(b"treasury"), Seed::from(&tb)],
    )?;
    emit(
        [143, 181, 157, 169, 87, 155, 170, 46],
        &[&amount.to_le_bytes(), account(a, 0)?.address().as_ref()],
    )
}

fn admin(a: &mut [AccountView], d: &[u8], mode: u8) -> ProgramResult {
    let mut c = config(account(a, 1)?)?;
    writable(account(a, 1)?)?;
    auth(
        &c,
        if mode == 4 { c::PENDING } else { c::ADMIN },
        account(a, 0)?,
    )?;
    match mode {
        0 => {
            let paused = byte(d, 0)?;
            need(paused <= 1, Error::BadParams)?;
            need(
                paused == 1 || byte(&c, c::CLOSING)? == 0,
                Error::WindingDown,
            )?;
            put(&mut c, c::PAUSED, &[paused])?;
            emit([238, 188, 213, 78, 134, 209, 178, 218], &[&[paused]])?;
        }
        1 | 3 => {
            let k = key(d, 0)?;
            put(
                &mut c,
                if mode == 1 { c::OPERATOR } else { c::PENDING },
                k.as_ref(),
            )?;
            emit(
                if mode == 1 {
                    [231, 79, 62, 226, 190, 139, 176, 51]
                } else {
                    [129, 249, 226, 227, 199, 82, 110, 243]
                },
                &[k.as_ref()],
            )?;
        }
        2 => {
            let secs = u32::from_le_bytes(rd(d, 0)?);
            delay(secs)?;
            put(&mut c, c::DELAY, &secs.to_le_bytes())?;
            emit([177, 68, 228, 40, 187, 251, 46, 35], &[&secs.to_le_bytes()])?;
        }
        4 => {
            let k = key(&c, c::PENDING)?;
            put(&mut c, c::ADMIN, k.as_ref())?;
            put(&mut c, c::PENDING, ZERO.as_ref())?;
            emit([174, 12, 76, 139, 158, 99, 110, 254], &[k.as_ref()])?;
        }
        _ => {
            need(byte(&c, c::PAUSED)? == 1, Error::NotPaused)?;
            put(&mut c, c::CLOSING, &[1])?;
            emit([61, 218, 238, 42, 156, 139, 201, 227], &[])?;
        }
    }
    save(a.get_mut(1).ok_or(ProgramError::NotEnoughAccountKeys)?, &c)
}

fn settle(a: &mut [AccountView], d: &[u8]) -> ProgramResult {
    let hash = rd::<32>(d, 0)?;
    let mut c = config(account(a, 1)?)?;
    auth(&c, c::ADMIN, account(a, 0)?)?;
    let p = product(account(a, 2)?, &hash)?;
    for i in 0..5 {
        writable(account(a, i)?)?;
    }
    system(account(a, 5)?)?;
    need(byte(&c, c::CLOSING)? == 1, Error::NotWindingDown)?;
    let vb = [byte(&p, p::VAULT_BUMP)?];
    money(account(a, 3)?, &[b"vault", &hash, &vb])?;
    let startup = key(&p, p::STARTUP)?;
    let tb = [byte(&c, c::TREASURY_BUMP)?];
    let to = if startup == ZERO {
        derived(&[b"treasury", &tb])?
    } else {
        startup
    };
    need(account(a, 4)?.address() == &to, Error::Unauthorized)?;
    if startup == ZERO {
        money(account(a, 4)?, &[b"treasury", &tb])?;
    }
    let amount = account(a, 3)?.lamports();
    transfer(
        account(a, 3)?,
        account(a, 4)?,
        amount,
        &[Seed::from(b"vault"), Seed::from(&hash), Seed::from(&vb)],
    )?;
    let open = sub(u64_at(&c, c::OPEN)?, 1)?;
    put(&mut c, c::OPEN, &open.to_le_bytes())?;
    save(a.get_mut(1).ok_or(ProgramError::NotEnoughAccountKeys)?, &c)?;
    close_account(a, 2, 0)?;
    emit(
        [232, 210, 40, 17, 142, 124, 145, 238],
        &[&hash, &amount.to_le_bytes(), to.as_ref()],
    )
}

fn close_config(a: &mut [AccountView]) -> ProgramResult {
    let c = config(account(a, 1)?)?;
    auth(&c, c::ADMIN, account(a, 0)?)?;
    for i in 0..3 {
        writable(account(a, i)?)?;
    }
    system(account(a, 3)?)?;
    let tb = [byte(&c, c::TREASURY_BUMP)?];
    money(account(a, 2)?, &[b"treasury", &tb])?;
    need(byte(&c, c::CLOSING)? == 1, Error::NotWindingDown)?;
    need(u64_at(&c, c::OPEN)? == 0, Error::ProductsOpen)?;
    let amount = account(a, 2)?.lamports();
    transfer(
        account(a, 2)?,
        account(a, 0)?,
        amount,
        &[Seed::from(b"treasury"), Seed::from(&tb)],
    )?;
    close_account(a, 1, 0)?;
    emit(
        [4, 138, 208, 218, 204, 236, 118, 199],
        &[&amount.to_le_bytes(), account(a, 0)?.address().as_ref()],
    )
}

fn read_coin(
    mint: &AccountView,
    sc: &AccountView,
    bc: &AccountView,
    vault: &Address,
    treasury: &Address,
) -> ProgramResult {
    // Map malformed/truncated external data to the existing pump error.
    read_coin_data(mint, sc, bc, vault, treasury).map_err(|e| {
        if e == ProgramError::InvalidAccountData {
            err(Error::BadPumpAccount)
        } else {
            e
        }
    })
}
fn read_coin_data(
    mint: &AccountView,
    sc: &AccountView,
    bc: &AccountView,
    vault: &Address,
    treasury: &Address,
) -> ProgramResult {
    need(
        mint.owned_by(&TOKEN) || mint.owned_by(&TOKEN_2022),
        Error::BadPumpAccount,
    )?;
    let _: [u8; 8] = rd(&mint.try_borrow()?, 36)?;
    need(
        sc.owned_by(&PUMP_FEES)
            && sc.address()
                == &Address::find_program_address(
                    &[b"sharing-config", mint.address().as_ref()],
                    &PUMP_FEES,
                )
                .0,
        Error::BadPumpAccount,
    )?;
    {
        let d = sc.try_borrow()?;
        need(rd::<8>(&d, 0)? == SC_DISC, Error::BadPumpAccount)?;
        need(byte(&d, 10)? == 1 && byte(&d, 75)? == 1, Error::BadShares)?;
        need(key(&d, 11)? == *mint.address(), Error::BadPumpAccount)?;
        need(u32::from_le_bytes(rd(&d, 76)?) == 2, Error::BadShares)?;
        let a = key(&d, 80)?;
        let ab = u16::from_le_bytes(rd(&d, 112)?);
        let b = key(&d, 114)?;
        let bb = u16::from_le_bytes(rd(&d, 146)?);
        need(
            (a == *vault && ab == 9000 && b == *treasury && bb == 1000)
                || (b == *vault && bb == 9000 && a == *treasury && ab == 1000),
            Error::BadShares,
        )?;
    }
    need(
        bc.owned_by(&PUMP)
            && bc.address()
                == &Address::find_program_address(
                    &[b"bonding-curve", mint.address().as_ref()],
                    &PUMP,
                )
                .0,
        Error::BadPumpAccount,
    )?;
    let d = bc.try_borrow()?;
    need(rd::<8>(&d, 0)? == BC_DISC, Error::BadPumpAccount)?;
    need(
        key(&d, 49)? == *sc.address() && byte(&d, 81)? == 0,
        Error::BadShares,
    )?;
    let quote = key(&d, 83)?;
    need(quote == ZERO || quote == WSOL, Error::BadShares)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pump_reads_are_bounded_and_delay_is_inclusive() {
        assert!(key(&[0; 31], 0).is_err());
        assert!(rd::<8>(&[0; 8], usize::MAX).is_err());
        assert!(byte(&[], 0).is_err());
        assert!(delay(3599).is_err());
        assert!(delay(3600).is_ok());
        assert!(delay(30 * 86400).is_ok());
        assert!(delay(30 * 86400 + 1).is_err());
    }
}
