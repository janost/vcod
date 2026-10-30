// vcod's compass probe for mp_crash, loaded over the map's own effects
// script: `ping` runs the stock `pingPlayer` on the player (the quick
// messages' call), `die` kills him through `suicide`.
main()
{
}

ping( unused )
{
	self pingPlayer();
}

die( unused )
{
	self suicide();
}
